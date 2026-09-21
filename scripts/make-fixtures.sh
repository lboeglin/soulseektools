#!/usr/bin/env bash
# Regenerate the committed audio fixtures.
#
# Everything the test suite can build in Rust it builds at test time; these are
# the files it cannot. There is no pure-Rust encoder for MP3, AAC, Vorbis or
# Opus, and the transcode detector needs audio a real lossy encoder produced --
# the brickwall it recognises is LAME's, and you cannot synthesise an authentic
# one. So these are encoded once, here, and committed.
#
# Needs ffmpeg. Run it only when a fixture has to change; the suite never does.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$here/tests/data/audio"
mkdir -p "$out"

tone="sine=frequency=440:duration=2:sample_rate=44100"
swept="aevalsrc=0.5*sin(2*PI*300*t)*(0.55+0.45*sin(2*PI*0.7*t)):d=8:s=44100"
noise="anoisesrc=d=8:c=white:r=44100"

encoder() {
  case "$1" in
    flac) echo "-c:a flac -sample_fmt s16" ;;
    mp3)  echo "-c:a libmp3lame -b:a 192k" ;;
    m4a)  echo "-c:a aac -b:a 192k" ;;
    ogg)  echo "-c:a libvorbis -q:a 4" ;;
    opus) echo "-c:a libopus -b:a 128k" ;;
    *)    echo "no encoder for $1" >&2; exit 1 ;;
  esac
}

# Two untagged seconds of tone per container the suite cannot encode, which
# `support::fixture` copies and then tags through lofty.
for extension in mp3 m4a ogg opus; do
  ffmpeg -v error -y -f lavfi -i "$tone" -ac 2 $(encoder "$extension") "$out/tone.$extension"
done

# The same audio carrying tags ffmpeg wrote, so the reader is still checked
# against a writer this project had no hand in.
for extension in flac mp3 m4a ogg opus; do
  ffmpeg -v error -y -f lavfi -i "$tone" -ac 2 $(encoder "$extension") \
    -metadata "artist=DECO*27" -metadata "album=Ghost" -metadata "title=Roki" \
    -metadata "genre=Vocaloid" -metadata "track=3" "$out/tagged.$extension"
done

# One recording in two containers, for the fingerprint comparison. Both come
# from this source, which is the whole point of committing the pair.
ffmpeg -v error -y -f lavfi -i "$swept" -ac 2 $(encoder flac) "$out/pair.flac"
ffmpeg -v error -y -f lavfi -i "$swept" -ac 2 $(encoder mp3) "$out/pair.mp3"

# Broadband noise through LAME at the bitrates the classifier keys on. The
# tests decode these and re-encode them as FLAC, which is the deception
# `stk verify` exists to catch.
for bitrate in 128k 192k 320k; do
  ffmpeg -v error -y -f lavfi -i "$noise" -ac 2 -c:a libmp3lame -b:a "$bitrate" \
    "$out/noise-$bitrate.mp3"
done

ls -l "$out"
