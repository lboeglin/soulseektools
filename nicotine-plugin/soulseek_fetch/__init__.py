"""
Soulseek Fetch plugin.

Reads a CSV wantlist (query column "Artist name" / "Track name"), searches
Soulseek for each track, ranks the results, and writes the top 3 candidates
per track into a review CSV. Nothing downloads automatically -- a human
fills in the "approve" column (1/2/3) picking which candidate to grab, and
the plugin polls that file and queues approved downloads.

Finished downloads are run through a spectral fake-FLAC check and the
verdict is logged.
"""
import csv
import difflib
import json
import os
import re
import subprocess
import time
import unicodedata

from pynicotine.events import events
from pynicotine.pluginsystem import BasePlugin
from pynicotine.slskmessages import FileAttribute

LOSSLESS_EXTS = {"flac", "wav", "ape", "alac", "wv", "aiff"}
LOSSY_EXTS = {"mp3", "ogg", "m4a", "aac", "opus", "wma"}
AUDIO_EXTS = LOSSLESS_EXTS | LOSSY_EXTS
TITLE_MATCH_THRESHOLD = 0.45

FEAT_RE = re.compile(r"\(feat\.[^)]*\)|feat\.[^,(\[]*", re.IGNORECASE)
PAREN_RE = re.compile(r"[\(\[][^\)\]]*[\)\]]")
PUNCT_RE = re.compile(r"[^\w\s]", re.UNICODE)
INVALID_FS_CHARS = re.compile(r'[<>:"/\\|?*]')


def normalize(text):
    text = unicodedata.normalize("NFKC", text or "")
    text = FEAT_RE.sub(" ", text)
    text = PAREN_RE.sub(" ", text)
    text = PUNCT_RE.sub(" ", text)
    return re.sub(r"\s+", " ", text).strip().lower()


def sanitize_folder_name(name):
    return INVALID_FS_CHARS.sub("_", name).strip() or "Unknown Artist"


class Plugin(BasePlugin):

    REVIEW_FIELDS = [
        "query", "artist", "title", "status",
        "cand1_score", "cand1_desc", "cand1_user", "cand1_path", "cand1_size",
        "cand1_bitrate", "cand1_samplerate", "cand1_bitdepth", "cand1_vbr",
        "cand2_score", "cand2_desc", "cand2_user", "cand2_path", "cand2_size",
        "cand2_bitrate", "cand2_samplerate", "cand2_bitdepth", "cand2_vbr",
        "cand3_score", "cand3_desc", "cand3_user", "cand3_path", "cand3_size",
        "cand3_bitrate", "cand3_samplerate", "cand3_bitdepth", "cand3_vbr",
        "approve",
    ]

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

        self.settings = {
            "csv_path": r"C:\Users\Jment\Downloads\missing.csv",
            "review_path": r"C:\Users\Jment\Downloads\soulseek_review.csv",
            "download_root": r"E:\Music",
            "search_wait_seconds": 15,
            "search_stagger_seconds": 25,
            "approval_poll_seconds": 30,
        }
        self.metasettings = {
            "csv_path": {"description": "Wantlist CSV (Track name / Artist name columns)", "type": "file"},
            "review_path": {"description": "Review CSV (candidates + approve column)", "type": "file"},
            "download_root": {"description": "Root folder downloads are sorted into (by artist)", "type": "folder"},
            "search_wait_seconds": {"description": "Seconds to collect results per search", "type": "int", "minimum": 5},
            "search_stagger_seconds": {"description": "Seconds between firing consecutive searches", "type": "int", "minimum": 1},
            "approval_poll_seconds": {"description": "How often to re-check the review file for new approvals", "type": "int", "minimum": 10},
        }

        self.commands = {
            "fetch-status": {
                "callback": self.fetch_status_command,
                "description": "Show Soulseek Fetch batch progress",
            },
        }

        self.queue = []
        self.pending_searches = {}
        self.total_tracks = 0
        self.done_tracks = 0
        self.batch_started = False

    def loaded_notification(self):
        self.checker_path = os.path.join(os.path.expanduser("~"), "Documents", "SoulseekTools", "check_fake_flac.py")
        self.log(f"Loaded. Checker: {self.checker_path}")

    def server_connect_notification(self):
        events.connect("file-search-response", self._on_search_response)
        # give shares/connection a moment to settle before hammering searches
        events.schedule(5, self._start_batch)
        events.schedule(10, self._process_approvals)

    # ---------------------------------------------------------------- search

    def _start_batch(self):
        if self.batch_started:
            return
        self.batch_started = True

        if not os.path.isfile(self.settings["csv_path"]):
            self.log(f"No wantlist found at {self.settings['csv_path']}, skipping search batch.")
            return

        already_done = set()
        if os.path.isfile(self.settings["review_path"]):
            with open(self.settings["review_path"], encoding="utf-8-sig", newline="") as f:
                for row in csv.DictReader(f):
                    already_done.add(row["query"])
        else:
            with open(self.settings["review_path"], "w", encoding="utf-8-sig", newline="") as f:
                csv.DictWriter(f, fieldnames=self.REVIEW_FIELDS).writeheader()

        with open(self.settings["csv_path"], encoding="utf-8-sig", newline="") as f:
            rows = list(csv.DictReader(f))

        self.queue = [r for r in rows if self._query_for(r) not in already_done]
        self.total_tracks = len(self.queue)
        self.done_tracks = 0

        self.log(f"Soulseek Fetch: {self.total_tracks} tracks to search "
                 f"({len(rows) - self.total_tracks} already in review file)")
        self._fire_next_search()

    @staticmethod
    def _query_for(row):
        return f"{row['Artist name']} - {row['Track name']}"

    def _fire_next_search(self):
        if not self.queue:
            if self.total_tracks:
                self.log(f"Soulseek Fetch: search batch complete ({self.done_tracks}/{self.total_tracks}). "
                         f"Review candidates in {self.settings['review_path']}")
            return

        row = self.queue.pop(0)
        artist = row["Artist name"]
        title = row["Track name"]
        query_text = normalize(f"{artist} {title}")

        self.core.search.do_search(query_text, mode="global", switch_page=False)
        token = self.core.search.token

        self.pending_searches[token] = {
            "row": row, "artist": artist, "title": title,
            "artist_norm": normalize(artist), "title_norm": normalize(title),
            "candidates": [],
        }

        events.schedule(self.settings["search_wait_seconds"], self._finalize_search, callback_args=(token,))
        events.schedule(self.settings["search_stagger_seconds"], self._fire_next_search)

    def _on_search_response(self, msg):
        entry = self.pending_searches.get(msg.token)
        if entry is None:
            return

        for _code, path, size, _ext, attrs in msg.list:
            entry["candidates"].append({
                "user": msg.username,
                "path": path,
                "size": size,
                "attrs": attrs,
                "freeulslots": msg.freeulslots,
                "inqueue": msg.inqueue,
            })

    def _score_candidate(self, candidate, artist_norm, title_norm):
        filename = candidate["path"].rsplit("\\", 1)[-1]
        ext = os.path.splitext(filename)[1].lstrip(".").lower()
        if ext not in AUDIO_EXTS:
            return None

        name_norm = normalize(os.path.splitext(filename)[0])

        title_ratio = difflib.SequenceMatcher(None, title_norm, name_norm).ratio()
        if title_norm and title_norm in name_norm:
            title_ratio = max(title_ratio, 0.8)
        artist_ratio = difflib.SequenceMatcher(None, artist_norm, name_norm).ratio()
        if artist_norm and artist_norm in name_norm:
            artist_ratio = max(artist_ratio, 0.8)

        if max(title_ratio, artist_ratio * 0.6) < TITLE_MATCH_THRESHOLD:
            return None

        attrs = candidate["attrs"] or {}
        bitrate = attrs.get(FileAttribute.BITRATE)
        vbr = attrs.get(FileAttribute.VBR)
        sample_rate = attrs.get(FileAttribute.SAMPLE_RATE)
        bit_depth = attrs.get(FileAttribute.BIT_DEPTH)

        is_lossless = ext in LOSSLESS_EXTS or bit_depth is not None
        if is_lossless:
            tier = 3
            quality = (sample_rate or 44100) * (bit_depth or 16)
        elif bitrate:
            tier = 2 if bitrate >= 256 else 1
            quality = bitrate
        else:
            tier = 0
            quality = 0

        score = (tier * 10_000_000) + (quality * 10)
        score += 1000 if candidate["freeulslots"] else 0
        score -= min(candidate["inqueue"] or 0, 500)
        score += int(title_ratio * 500)

        desc = f"{filename} [{ext or '?'}"
        if is_lossless and sample_rate and bit_depth:
            desc += f", {sample_rate/1000:.1f}kHz/{bit_depth}bit"
        elif bitrate:
            desc += f", {bitrate}kbps{'VBR' if vbr else ''}"
        desc += f"] user={candidate['user']} queue={candidate['inqueue']} slot={'free' if candidate['freeulslots'] else 'busy'}"

        return {
            "score": score, "desc": desc, "user": candidate["user"], "path": candidate["path"],
            "size": candidate["size"], "bitrate": bitrate or "", "samplerate": sample_rate or "",
            "bitdepth": bit_depth or "", "vbr": vbr or "",
        }

    def _finalize_search(self, token):
        entry = self.pending_searches.pop(token, None)
        if entry is None:
            return

        self.done_tracks += 1
        scored = []
        for candidate in entry["candidates"]:
            result = self._score_candidate(candidate, entry["artist_norm"], entry["title_norm"])
            if result:
                scored.append(result)
        scored.sort(key=lambda r: r["score"], reverse=True)
        top3 = scored[:3]

        row_out = {
            "query": self._query_for(entry["row"]), "artist": entry["artist"], "title": entry["title"],
            "status": "", "approve": "",
        }
        for i in range(3):
            prefix = f"cand{i + 1}_"
            if i < len(top3):
                c = top3[i]
                row_out[prefix + "score"] = c["score"]
                row_out[prefix + "desc"] = c["desc"]
                row_out[prefix + "user"] = c["user"]
                row_out[prefix + "path"] = c["path"]
                row_out[prefix + "size"] = c["size"]
                row_out[prefix + "bitrate"] = c["bitrate"]
                row_out[prefix + "samplerate"] = c["samplerate"]
                row_out[prefix + "bitdepth"] = c["bitdepth"]
                row_out[prefix + "vbr"] = c["vbr"]
            else:
                for suffix in ("score", "desc", "user", "path", "size", "bitrate", "samplerate", "bitdepth", "vbr"):
                    row_out[prefix + suffix] = ""

        with open(self.settings["review_path"], "a", encoding="utf-8-sig", newline="") as f:
            csv.DictWriter(f, fieldnames=self.REVIEW_FIELDS).writerow(row_out)

        self.log(f"[{self.done_tracks}/{self.total_tracks}] {entry['artist']} - {entry['title']}: "
                 f"{len(top3)} candidate(s)" + (f" -> best: {top3[0]['desc']}" if top3 else " -> NO MATCH"))

    def fetch_status_command(self, _args):
        self.output(f"Soulseek Fetch: {self.done_tracks}/{self.total_tracks} searched, "
                    f"{len(self.pending_searches)} in flight, {len(self.queue)} queued.")

    # ------------------------------------------------------------ approvals

    def _process_approvals(self):
        events.schedule(self.settings["approval_poll_seconds"], self._process_approvals)

        path = self.settings["review_path"]
        if not os.path.isfile(path):
            return

        with open(path, encoding="utf-8-sig", newline="") as f:
            rows = list(csv.DictReader(f))

        changed = False
        for row in rows:
            approve = (row.get("approve") or "").strip()
            if approve not in {"1", "2", "3"} or row.get("status") == "queued":
                continue

            prefix = f"cand{approve}_"
            user = row.get(prefix + "user")
            virtual_path = row.get(prefix + "path")
            if not user or not virtual_path:
                row["status"] = "error: candidate empty"
                changed = True
                continue

            size = int(row.get(prefix + "size") or 0)
            attrs = {}
            if row.get(prefix + "bitrate"):
                attrs[FileAttribute.BITRATE] = int(row[prefix + "bitrate"])
            if row.get(prefix + "samplerate"):
                attrs[FileAttribute.SAMPLE_RATE] = int(row[prefix + "samplerate"])
            if row.get(prefix + "bitdepth"):
                attrs[FileAttribute.BIT_DEPTH] = int(row[prefix + "bitdepth"])
            if row.get(prefix + "vbr"):
                attrs[FileAttribute.VBR] = int(row[prefix + "vbr"])

            folder = os.path.join(self.settings["download_root"], sanitize_folder_name(row["artist"]))
            self.core.downloads.enqueue_download(user, virtual_path, folder_path=folder, size=size,
                                                  file_attributes=attrs)
            row["status"] = "queued"
            changed = True
            self.log(f"Queued download: {row['artist']} - {row['title']} (candidate {approve}) -> {folder}")

        if changed:
            with open(path, "w", encoding="utf-8-sig", newline="") as f:
                writer = csv.DictWriter(f, fieldnames=self.REVIEW_FIELDS)
                writer.writeheader()
                writer.writerows(rows)

    # ------------------------------------------------------------- downloads

    def download_finished_notification(self, user, virtual_path, real_path):
        ext = os.path.splitext(real_path)[1].lstrip(".").lower()
        if ext not in LOSSLESS_EXTS:
            return

        if not os.path.isfile(self.checker_path):
            return

        try:
            proc = subprocess.run(
                ["python", self.checker_path, "--file", real_path],
                capture_output=True, text=True, timeout=120,
            )
            result = json.loads(proc.stdout)
        except Exception as e:
            self.log(f"Fake-FLAC check failed for {real_path}: {e}")
            return

        verdict = result.get("verdict")
        if verdict == "OK":
            self.log(f"Fake-FLAC check OK: {os.path.basename(real_path)}")
        else:
            self.log(f"*** Fake-FLAC check {verdict}: {os.path.basename(real_path)} -- {result.get('detail')}")
