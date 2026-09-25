#!/bin/sh
# Fake yt-dlp reporting a version below the floor this build supports (RD-102-03).
# Everything else answers exactly as the normal fixture does, so a test that reaches the
# probe or the runner proves the version gate stopped it and not a broken script.
if [ "$1" = "--version" ]; then echo "2019.01.01"; exit 0; fi
exec "$(dirname "$0")/fake-yt-dlp.sh" "$@"
