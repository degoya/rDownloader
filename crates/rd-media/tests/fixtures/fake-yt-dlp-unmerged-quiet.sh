#!/bin/sh
FAKE_YTDLP_MODE=unmerged-quiet exec "$(dirname "$0")/fake-yt-dlp.sh" "$@"
