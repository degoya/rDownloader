#!/bin/sh
FAKE_YTDLP_MODE=slow exec "$(dirname "$0")/fake-yt-dlp.sh" "$@"
