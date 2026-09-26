#!/bin/sh
FAKE_YTDLP_MODE=unmerged exec "$(dirname "$0")/fake-yt-dlp.sh" "$@"
