#!/bin/sh
FAKE_YTDLP_MODE=fail exec "$(dirname "$0")/fake-yt-dlp.sh" "$@"
