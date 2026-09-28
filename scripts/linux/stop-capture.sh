#!/usr/bin/env bash
# Stops only the capture agent. Same as "stop-rdownloader.sh capture".
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/stop-rdownloader.sh" capture
