#!/usr/bin/env bash
# Regenerate tests/data/audio: the lossy fixtures Rust can't encode. Needs ffmpeg.
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

# Untagged tone, which the tests copy and tag through lofty.
for extension in mp3 m4a ogg opus; do
  ffmpeg -v error -y -f lavfi -i "$tone" -ac 2 $(encoder "$extension") "$out/tone.$extension"
done

# Tags written by ffmpeg, so the reader is checked against another writer.
for extension in flac mp3 m4a ogg opus; do
  ffmpeg -v error -y -f lavfi -i "$tone" -ac 2 $(encoder "$extension") \
    -metadata "artist=DECO*27" -metadata "album=Ghost" -metadata "title=Roki" \
    -metadata "genre=Vocaloid" -metadata "track=3" "$out/tagged.$extension"
done

# One recording in two containers, for fingerprint comparison.
ffmpeg -v error -y -f lavfi -i "$swept" -ac 2 $(encoder flac) "$out/pair.flac"
ffmpeg -v error -y -f lavfi -i "$swept" -ac 2 $(encoder mp3) "$out/pair.mp3"

# Noise through LAME at the bitrates the transcode check knows.
for bitrate in 128k 192k 320k; do
  ffmpeg -v error -y -f lavfi -i "$noise" -ac 2 -c:a libmp3lame -b:a "$bitrate" \
    "$out/noise-$bitrate.mp3"
done

# A genuine VBR encode, and the 128k file re-encoded at 320k (CBR and ABR): an
# upsampled MP3. ABR, because V0 of a band-limited source averages too low.
ffmpeg -v error -y -f lavfi -i "$noise" -ac 2 -c:a libmp3lame -q:a 0 "$out/noise-v0.mp3"
ffmpeg -v error -y -i "$out/noise-128k.mp3" -c:a libmp3lame -b:a 320k "$out/noise-128k-as-320k.mp3"
ffmpeg -v error -y -i "$out/noise-128k.mp3" -c:a libmp3lame -abr 1 -b:a 320k "$out/noise-128k-as-vbr320.mp3"

# 44.1 kHz noise through ffmpeg's default (soft) resampler to 96 kHz, quiet
# enough not to clip.
ffmpeg -v error -y -f lavfi -i "anoisesrc=d=1.5:c=white:r=44100:a=0.25" \
  -af aresample=96000 -c:a flac -sample_fmt s16 "$out/noise-44k-as-96k.flac"

ls -l "$out"
