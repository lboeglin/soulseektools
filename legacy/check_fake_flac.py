"""
Detect "fake" lossless files (FLAC/WAV/ALAC transcoded up from a lossy
source like MP3/AAC) by looking at where the actual audio content cuts off
in the frequency spectrum.

A real CD-sourced lossless file has content out to close to the Nyquist
frequency (sample_rate / 2). A file that was really an MP3/AAC re-encoded
as FLAC keeps the lossy encoder's cutoff baked in -- typically:
  ~16 kHz  -> source was roughly 128 kbps
  ~17-18kHz -> source was roughly 192 kbps
  ~19-20kHz -> source was roughly 256-320 kbps
A hard, clean cutoff at one of these points (rather than a gradual natural
roll-off) is the signature of a transcode.

Usage:
    python check_fake_flac.py <folder> [--report out.csv] [--spectrograms]

Requires ffmpeg/ffprobe on PATH and numpy installed.
"""
import argparse
import csv
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

AUDIO_EXTS = {".flac", ".wav", ".m4a", ".alac", ".ape", ".wv"}

# Only meaningful to check "lossless" containers -- an mp3 is honestly lossy,
# nothing to detect.
NOISE_FLOOR_DB = -50.0   # bands quieter than this (rel. to peak) count as silence
BAND_WIDTH_HZ = 150
NEAR_NYQUIST_MARGIN_HZ = 1500

KNOWN_LOSSY_CUTOFFS = [
    (15500, 16500, "~128kbps source"),
    (17000, 18500, "~192kbps source"),
    (18800, 20200, "~256-320kbps source"),
]


def ffprobe_info(path):
    out = subprocess.run(
        ["ffprobe", "-v", "quiet", "-print_format", "json", "-show_streams", str(path)],
        capture_output=True, text=True, check=True,
    )
    data = json.loads(out.stdout)
    stream = next(s for s in data["streams"] if s["codec_type"] == "audio")
    return {
        "sample_rate": int(stream["sample_rate"]),
        "bits_per_raw_sample": stream.get("bits_per_raw_sample") or stream.get("bits_per_sample"),
        "codec": stream["codec_name"],
    }


def decode_pcm(path, sample_rate, seconds_per_probe=20, num_probes=4, duration=None):
    """Decode a handful of ~seconds_per_probe-second windows spread across
    the track (mono, native sample rate, float32) instead of the whole file,
    for speed on large libraries."""
    if duration is None:
        out = subprocess.run(
            ["ffprobe", "-v", "quiet", "-show_entries", "format=duration",
             "-of", "csv=p=0", str(path)],
            capture_output=True, text=True, check=True,
        )
        duration = float(out.stdout.strip() or 0)

    chunks = []
    if duration <= seconds_per_probe * num_probes:
        starts = [0]
        seconds_per_probe = max(duration - 1, 1)
    else:
        margin = duration * 0.1
        span = duration - 2 * margin
        starts = [margin + span * i / max(num_probes - 1, 1) for i in range(num_probes)]

    for start in starts:
        proc = subprocess.run(
            ["ffmpeg", "-v", "quiet", "-ss", str(start), "-t", str(seconds_per_probe),
             "-i", str(path), "-f", "f32le", "-ac", "1", "-ar", str(sample_rate), "-"],
            capture_output=True, check=True,
        )
        chunk = np.frombuffer(proc.stdout, dtype=np.float32)
        if len(chunk):
            chunks.append(chunk)
    return chunks


def spectrum_db(chunks, sample_rate, fft_size=8192):
    window = np.hanning(fft_size)
    accum = None
    count = 0
    for chunk in chunks:
        for i in range(0, len(chunk) - fft_size, fft_size // 2):
            frame = chunk[i:i + fft_size] * window
            mag = np.abs(np.fft.rfft(frame))
            accum = mag if accum is None else accum + mag
            count += 1
    if count == 0 or accum is None:
        return None, None
    avg_mag = accum / count
    freqs = np.fft.rfftfreq(fft_size, d=1.0 / sample_rate)
    db = 20 * np.log10(avg_mag + 1e-12)
    db -= db.max()
    return freqs, db


def band_average(freqs, db, band_width=BAND_WIDTH_HZ):
    max_f = freqs[-1]
    bands = np.arange(0, max_f, band_width)
    band_db = []
    for b in bands:
        mask = (freqs >= b) & (freqs < b + band_width)
        if mask.any():
            band_db.append(db[mask].mean())
        else:
            band_db.append(-999)
    return bands, np.array(band_db)


def find_cutoff(bands, band_db, noise_floor=NOISE_FLOOR_DB):
    above = np.where(band_db > noise_floor)[0]
    if len(above) == 0:
        return 0
    return float(bands[above[-1]])


def cliff_steepness(bands, band_db, cutoff_hz, window_hz=800):
    """Max dB drop per kHz found across any narrow window straddling the
    cutoff region. A real lossy-encoder cutoff is a near-vertical brickwall
    over a couple hundred Hz; a natural mastering roll-off is spread over
    several kHz. Scanning for the steepest local slope (rather than one
    fixed wide span) avoids diluting a real narrow cliff."""
    lo_f = max(cutoff_hz - window_hz * 2, 0)
    hi_f = cutoff_hz + window_hz
    lo_idx = np.searchsorted(bands, lo_f)
    hi_idx = np.searchsorted(bands, hi_f)
    hi_idx = min(hi_idx, len(band_db) - 1)
    if hi_idx <= lo_idx + 1:
        return 0.0

    best = 0.0
    step = max(1, window_hz // BAND_WIDTH_HZ)
    for i in range(lo_idx, hi_idx - step):
        drop = band_db[i] - band_db[i + step]
        khz = (step * BAND_WIDTH_HZ) / 1000.0
        rate = drop / khz
        if rate > best:
            best = rate
    return best


STEEP_DB_PER_KHZ = 25.0  # brickwall transcode cutoffs are typically 40-100+ dB/kHz measured this narrow; natural roll-off rarely exceeds ~15-20


def classify(cutoff_hz, nyquist_hz, steepness):
    if cutoff_hz >= nyquist_hz - NEAR_NYQUIST_MARGIN_HZ:
        return "OK", "content extends near Nyquist limit -- looks genuinely lossless"

    is_steep = steepness >= STEEP_DB_PER_KHZ
    matched_label = None
    for lo, hi, label in KNOWN_LOSSY_CUTOFFS:
        if lo <= cutoff_hz <= hi:
            matched_label = label
            break

    if is_steep and matched_label:
        return "FAKE", f"hard brickwall cutoff at {cutoff_hz:.0f}Hz ({steepness:.0f}dB/kHz) matches {matched_label} -- likely transcoded"
    if is_steep:
        return "SUSPECT", f"steep cutoff at {cutoff_hz:.0f}Hz ({steepness:.0f}dB/kHz) -- doesn't match a standard bitrate but the sharpness looks artificial, inspect manually"
    return "OK", f"gradual roll-off starting ~{cutoff_hz:.0f}Hz ({steepness:.0f}dB/kHz) -- consistent with normal mastering, not a hard transcode cutoff"


def analyze_file(path):
    info = ffprobe_info(path)
    sample_rate = info["sample_rate"]
    nyquist = sample_rate / 2
    chunks = decode_pcm(path, sample_rate)
    freqs, db = spectrum_db(chunks, sample_rate)
    if freqs is None:
        return {"path": str(path), "verdict": "ERROR", "detail": "could not decode audio"}
    bands, band_db = band_average(freqs, db)
    cutoff = find_cutoff(bands, band_db)
    steepness = cliff_steepness(bands, band_db, cutoff)
    verdict, detail = classify(cutoff, nyquist, steepness)

    # extra flag: file claims hi-res (48kHz+) but content doesn't even fill
    # out standard CD range with a steep, artificial-looking cutoff --
    # stronger sign of fake hi-res upsampling (gradual roll-off alone is
    # not enough evidence here either)
    if sample_rate >= 48000 and cutoff < 20500 and steepness >= STEEP_DB_PER_KHZ:
        verdict = "FAKE"
        detail = f"{sample_rate}Hz container but content cuts off sharply at {cutoff:.0f}Hz ({steepness:.0f}dB/kHz) -- fake hi-res (upsampled)"

    return {
        "path": str(path),
        "sample_rate": sample_rate,
        "bit_depth": info["bits_per_raw_sample"],
        "codec": info["codec"],
        "cutoff_hz": round(cutoff),
        "nyquist_hz": round(nyquist),
        "steepness_db_per_khz": round(steepness, 1),
        "verdict": verdict,
        "detail": detail,
    }


def make_spectrogram(path, out_png):
    subprocess.run(
        ["ffmpeg", "-v", "quiet", "-y", "-i", str(path),
         "-lavfi", "showspectrumpic=s=1024x512:legend=1",
         str(out_png)],
        check=True,
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("folder", nargs="?", help="folder to scan recursively")
    ap.add_argument("--file", default=None, help="analyze a single file and print JSON to stdout")
    ap.add_argument("--report", default=None, help="write CSV report here")
    ap.add_argument("--spectrograms", action="store_true",
                     help="save a spectrogram PNG next to each FAKE/SUSPECT file")
    args = ap.parse_args()

    if args.file:
        result = analyze_file(Path(args.file))
        print(json.dumps(result))
        return

    if not args.folder:
        ap.error("folder is required unless --file is given")

    root = Path(args.folder)
    files = [p for p in root.rglob("*") if p.suffix.lower() in AUDIO_EXTS]
    print(f"Scanning {len(files)} lossless-container files under {root}\n")

    results = []
    for i, path in enumerate(files, 1):
        print(f"[{i}/{len(files)}] {path.relative_to(root)} ... ", end="", flush=True)
        try:
            result = analyze_file(path)
        except subprocess.CalledProcessError as e:
            result = {"path": str(path), "verdict": "ERROR", "detail": str(e)}
        results.append(result)
        print(result["verdict"], "-", result.get("detail", ""))

        if args.spectrograms and result["verdict"] in ("FAKE", "SUSPECT"):
            out_png = path.with_suffix(".spectrogram.png")
            try:
                make_spectrogram(path, out_png)
                print(f"    spectrogram saved: {out_png}")
            except subprocess.CalledProcessError:
                pass

    fake = [r for r in results if r["verdict"] == "FAKE"]
    suspect = [r for r in results if r["verdict"] == "SUSPECT"]
    ok = [r for r in results if r["verdict"] == "OK"]
    print(f"\n{'='*60}")
    print(f"OK: {len(ok)}   SUSPECT: {len(suspect)}   FAKE: {len(fake)}")

    if args.report:
        with open(args.report, "w", newline="", encoding="utf-8-sig") as f:
            writer = csv.DictWriter(f, fieldnames=["path", "sample_rate", "bit_depth", "codec",
                                                     "cutoff_hz", "nyquist_hz", "steepness_db_per_khz",
                                                     "verdict", "detail"])
            writer.writeheader()
            writer.writerows(results)
        print(f"Report written to {args.report}")


if __name__ == "__main__":
    main()
