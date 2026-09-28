#!/usr/bin/env bash
# Starts only the capture agent, for a Mac whose rDownloader server runs elsewhere -- a NAS,
# a Docker host. Pair it once: that server's web interface shows the command under Settings >
# Desktop client. Same as "start-rdownloader.command capture"; this file exists to be double-clicked.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/start-rdownloader.command" capture
