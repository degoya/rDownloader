#!/bin/sh
# Fake yt-dlp for tests. Modes via FAKE_YTDLP_MODE: ok (default) | fail | slow.
mode="${FAKE_YTDLP_MODE:-ok}"
if [ "$1" = "--version" ]; then echo "2026.01.01-fake"; exit 0; fi
# This fixture doubles as the fake ffmpeg, which is asked with a single dash. Answering
# it here keeps a version probe from falling through into the download branch, where the
# empty output path made the script write into the crate directory (RD-102-03).
if [ "$1" = "-version" ]; then echo "ffmpeg version 2026.01.01-fake Copyright (c) fake"; exit 0; fi
if [ "$mode" = "fail" ]; then echo "ERROR: [youtube] abc: Video unavailable" >&2; exit 1; fi
if [ "$mode" = "slow" ]; then sleep 30; exit 0; fi
url=""
flat=0
output=""
audio=0
progress=0
printing=0
merged=0
for arg in "$@"; do
  case "$arg" in
    http*) url="$arg" ;;
    --flat-playlist) flat=1 ;;
    --extract-audio) audio=1 ;;
    --progress) progress=1 ;;
    --print) printing=1 ;;
    *+*) merged=1 ;;
  esac
done
# `--print after_move:rdownloader-final-path:%(filepath)s` is what the runner asks for, so the
# path comes back carrying that marker. Without `--print`, yt-dlp says nothing about the file.
print_final_path() {
  if [ "$printing" = "1" ]; then
    echo "rdownloader-final-path:$1"
  fi
}

prev=""
cookies=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then output="$arg"; fi
  if [ "$prev" = "--cookies" ]; then cookies="$arg"; fi
  prev="$arg"
done
case " $* " in
  *" -J "*)
    if [ "$flat" = "1" ]; then
      printf '%s' '{"_type":"playlist","title":"List","entries":[{"title":"First","url":"https://www.youtube.com/watch?v=one","duration":10},{"title":"Second","id":"two","duration":20}]}'
    elif echo "$url" | grep -q "list="; then
      printf '%s' '{"_type":"playlist","title":"List","entries":[]}'
    else
      printf '%s' '{"title":"Idle Immortal / Trailer","duration":95.4,"uploader":"Someone","thumbnail":"https://i.example/t.jpg","webpage_url":"https://www.youtube.com/watch?v=abc","formats":[{"format_id":"137","height":1080,"vcodec":"avc1","acodec":"none","ext":"mp4","filesize":5000},{"format_id":"22","height":720,"vcodec":"avc1","acodec":"mp4a","ext":"mp4","filesize":3000},{"format_id":"251","vcodec":"none","acodec":"opus","abr":160.0,"filesize":900}]}'
    fi
    exit 0 ;;
esac
# download mode
target=$(printf '%s' "$output" | sed 's/%(ext)s/mp4/')
if [ "$audio" = "1" ]; then target=$(printf '%s' "$output" | sed 's/%(ext)s/mp3/'); fi
# Lets tests assert which flags the runner actually passed.
printf '%s\n' "$*" > "$(dirname "$target")/ytdlp-args.txt"
# The cookie file is deleted the moment the download returns, so a test can only inspect
# what was handed over by copying it here while the process still holds it.
if [ -n "$cookies" ] && [ -f "$cookies" ]; then
  cp "$cookies" "$(dirname "$target")/ytdlp-cookies.txt"
fi
# Real yt-dlp turns quiet on for --print and then emits no progress unless --progress is
# also given. Mirroring that here keeps the runner honest about passing both.
if [ "$printing" = "0" ] || [ "$progress" = "1" ]; then
  if [ "$merged" = "1" ]; then
    # A merged format arrives as two streams that each count from 0 to 100 %.
    echo "[download] Destination: ${target%.*}.f137.mp4"
    echo "[download]  50.0% of  200.00KiB at 1.00MiB/s ETA 00:01"
    echo "[download] 100% of 200.00KiB in 00:00"
    echo "[download] Destination: ${target%.*}.f251.webm"
    echo "[download]  50.0% of  100.00KiB at 1.00MiB/s ETA 00:00"
    echo "[download] 100% of 100.00KiB in 00:00"
    echo "[Merger] Merging formats into \"$target\""
    head -c 307200 /dev/zero > "$target"
    print_final_path "$target"
    exit 0
  fi
  echo "[download] Destination: $target"
  echo "[download]  10.0% of  100.00KiB at 1.00MiB/s ETA 00:01"
  sleep 0.05
  echo "[download]  60.0% of  100.00KiB at 1.00MiB/s ETA 00:00"
  echo "[download] 100% of 100.00KiB in 00:00"
fi
head -c 102400 /dev/zero > "$target"
print_final_path "$target"
exit 0
