#!/usr/bin/env bash
# Starts only the capture agent, for a machine whose rDownloader server runs elsewhere -- a NAS,
# a Docker host. Pair it once: that server's web interface shows the command under Settings >
# Desktop client. Same as "start-rdownloader.sh capture"; this file exists to be started on its own.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/start-rdownloader.sh" capture
