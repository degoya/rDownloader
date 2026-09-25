#!/bin/sh
#
# Adopts the host's user id before starting the service.
#
# A bind mount belongs to a user on the host, and unless the id inside the container matches,
# every write into it fails. PUID/PGID is the convention people arriving from Sonarr, Radarr or
# anything else on a NAS already expect, so it is what this image speaks.
set -eu

PUID="${PUID:-10001}"
PGID="${PGID:-10001}"

# An explicit `user:` in compose already chose an identity. Changing it is neither possible nor
# wanted, and the entrypoint must not fail over it.
if [ "$(id -u)" != "0" ]; then
    exec rdownloader "$@"
fi

if [ "$PGID" != "$(id -g rdownloader)" ]; then
    groupmod -o -g "$PGID" rdownloader
fi
if [ "$PUID" != "$(id -u rdownloader)" ]; then
    usermod -o -u "$PUID" rdownloader
fi

# /config holds the database and the installed plugins and is small, so taking ownership of it
# outright is cheap and saves a first start that cannot write anything.
chown -R "$PUID:$PGID" /config

# /downloads gets the mount point itself and nothing below it. Docker creates a fresh named
# volume with the image's ownership, so without this the service cannot write a single file
# into its own download directory. Recursing would be a different matter: /downloads is
# routinely a NAS share with terabytes on it, and walking it would add minutes to every start.
# Files already in there keep their owner, which is the operator's business, not ours.
chown "$PUID:$PGID" /downloads

exec gosu "$PUID:$PGID" rdownloader "$@"
