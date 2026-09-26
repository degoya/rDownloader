# rDownloader in Docker

The container image is the whole service: REST API, web interface, scheduler and every
transport. It does not contain the desktop capture agent — for Click'n'Load and clipboard
capture on a headless setup, use the [browser extension](../extension/README.md) instead.

Images are published for `linux/amd64` and `linux/arm64`, signed with cosign, and carry an SBOM
and build provenance.

## Quick start

```bash
docker compose -f docker/compose.yml up -d
```

Then open <http://127.0.0.1:8710> and work through the setup wizard: an administrator password,
at least one storage root, and a category.

**The first start takes about a minute.** The service installs the bundled plugin packages,
loads the resolver components and the intake parsers before it begins listening. That is why the
health check has a 90-second start period. `docker logs -f rdownloader` shows the progress; the
line to wait for is `rDownloader listening`.

## Running the published image

```bash
docker run -d --name rdownloader \
  -p 127.0.0.1:8710:8710 \
  -v rdownloader-config:/config \
  -v /srv/media/movies:/media/movies \
  -e PUID=1000 -e PGID=1000 -e TZ=Europe/Berlin \
  ghcr.io/<owner>/rdownloader:latest
```

Verify the signature before you trust it:

```bash
cosign verify ghcr.io/<owner>/rdownloader:latest \
  --certificate-identity-regexp 'https://github.com/<owner>/rdownloader/.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

## Volumes and paths

| Path | What it holds | Back up |
| --- | --- | --- |
| `/config` | SQLite database, installed plugins, encrypted credentials, `vendor/` | **Yes** |
| `/downloads` | Fallback destination for a download without a category | Your call |

### Mount every path you use as a storage root

This is the one mistake that costs data, so it is worth being explicit about.

`/downloads` is **not** hard-wired. It is only the fallback destination the scheduler uses when
a download has no category. Storage roots are free absolute paths, and you are meant to have
several — `/media/movies`, `/media/series`, `/media/music`.

If you configure a storage root at a path that is **not** mounted, the directory is created
inside the container's writable layer. Downloads run normally, everything looks healthy, and all
of it is deleted the moment the container is removed or recreated — which includes every image
update.

rDownloader detects this and says so: such a root gets a **NOT PERSISTENT** badge in
*Settings → Routing*, an explanation above the form, and a note on the readiness card in
*Settings → System*. It is a warning, not a refusal, so a deliberate throwaway setup still
works. But if you see that badge, fix the mount before you download anything you want to keep.

So: for every storage root you intend to configure, add a volume first.

```yaml
volumes:
  - ./config:/config
  - /srv/media/movies:/media/movies
  - /srv/media/series:/media/series
```

Then create the storage roots inside the application pointing at `/media/movies` and
`/media/series`.

## PUID, PGID and file ownership

A bind mount belongs to a user on the host. Unless the user inside the container has the same
numeric id, every write into it fails.

Find your ids on the host and pass them in:

```bash
id -u   # -> PUID
id -g   # -> PGID
```

The entrypoint starts as root, adopts those ids, and drops to that user with `gosu` before the
service starts. This is the same convention Sonarr, Radarr and the linuxserver images use.

Two consequences worth knowing:

- **Mounted folders are not chowned recursively.** `/config` is taken over outright, because it
  is small. `/downloads` gets only the mount point itself — enough for a fresh named volume to
  be writable, without walking a NAS share with terabytes on it, which would add minutes to
  every start. Files already inside keep their owner. For any *additional* mount such as
  `/media/movies`, nothing is changed at all: make sure it is writable by `PUID:PGID` on the
  host, which it usually already is, since it is your own folder.
- **`no-new-privileges` cannot be used** together with PUID/PGID, because the entrypoint needs
  root to change the ids. If you would rather never run as root, set `user: "1000:1000"` in
  compose instead. The entrypoint notices it is already unprivileged and starts the service
  directly. In that case use **bind mounts, not named volumes**, for `/config`: Docker
  initialises a fresh named volume with the image's ownership (uid 10001), which your overridden
  user then cannot write to.

## Time zone

`TZ` defaults to `UTC`. Schedules, quiet hours, bandwidth windows and queue-completion actions
all run on the container's clock, so set it to your own zone or everything fires at the wrong
time:

```yaml
environment:
  TZ: "Europe/Berlin"
```

## External tools in the image

The image ships the helper binaries the service shells out to, so media downloads, streams,
galleries, unpacking, PAR2 repair and Apprise notifications work out of the box:

| Tool | Source | Used for |
| --- | --- | --- |
| `ffmpeg`, `ffprobe` | Debian | Merging and converting media, probing formats |
| `yt-dlp` | pip, pinned | Media downloads |
| `streamlink` | pip, pinned | Live streams |
| `gallery-dl` | pip, pinned | Galleries |
| `apprise` | pip, pinned | Notifications to Telegram, Discord, Matrix, ntfy and the like |
| `7z` (7-Zip `7zz`) | Debian | Unpacking, including RAR5 |
| `par2` | Debian | Usenet PAR2 verification and repair |

`unrar` is deliberately absent: it is non-free, and Debian's `unrar-free` cannot read RAR5.
`7zz` covers RAR extraction, and the image symlinks it to the name `7z` that the service looks
for.

The pinned versions are visible under *Settings → Tools*, together with where each
binary was found. **yt-dlp ages fast** — sites break and a fix lands within days. To run a newer
one without waiting for a new image, drop it into the vendor folder, which is searched before
`PATH`:

```bash
docker exec rdownloader mkdir -p /config/vendor
# copy your own yt-dlp into ./config/vendor on the host
```

`rclone` is not in the image. If you use the upload storage plugin, mount or copy an `rclone`
binary into `/config/vendor` the same way.

## Setting it up on a Synology NAS

DSM 7.2 or newer, using **Container Manager** (called Docker on older DSM versions).

### 1. Prepare the folders

In *File Station*, create the folders you will mount. A typical layout:

```
/volume1/docker/rdownloader/config
/volume1/media/movies
/volume1/media/series
```

### 2. Find the user ids

Enable SSH in *Control Panel → Terminal & SNMP*, log in, and run:

```bash
id -u your-dsm-user   # PUID
id -g your-dsm-user   # PGID
```

On DSM the first user is usually `1026`, and the `users` group is `100`. Do **not** guess —
wrong ids are the single most common reason downloads fail with permission errors on a Synology.

Make sure that user can write the folders: *File Station → right-click the folder →
Properties → Permission*.

### 3. Create the container

*Container Manager → Project → Create*, choose the folder
`/volume1/docker/rdownloader`, and paste this as the compose file:

```yaml
services:
  rdownloader:
    image: ghcr.io/<owner>/rdownloader:latest
    container_name: rdownloader
    restart: unless-stopped
    ports:
      - "8710:8710"
    volumes:
      - /volume1/docker/rdownloader/config:/config
      - /volume1/media/movies:/media/movies
      - /volume1/media/series:/media/series
    environment:
      PUID: "1026"
      PGID: "100"
      TZ: "Europe/Berlin"
```

Adjust `PUID`, `PGID` and the paths to what step 2 gave you.

### 4. First start

Open `http://<nas-ip>:8710`. Give it a minute — the plugin installation on NAS hardware is
noticeably slower than on a workstation, and Container Manager may show the container as
unhealthy until the start period is over.

In the wizard, create your storage roots at **`/media/movies` and `/media/series`** — the paths
*inside* the container, not the DSM paths. If you type a DSM path such as
`/volume1/media/movies`, it is not mounted inside the container, and rDownloader will badge it
**NOT PERSISTENT**: it would write into the container layer and lose everything on the next
image update.

### 5. Reaching it from outside

Do not forward port 8710 in your router. Use the DSM reverse proxy instead
(*Control Panel → Login Portal → Advanced → Reverse Proxy*), point a hostname at
`localhost:8710`, and give it a certificate from *Control Panel → Security → Certificate*. That
way the connection is encrypted and DSM handles the certificate renewal.

### Synology notes

- **Ports below 1024 are taken by DSM.** 8710 is free; if it clashes with something else, change
  the left-hand side of the port mapping only.
- **Btrfs snapshots of `/volume1/docker` will include the SQLite database.** That is fine for a
  stopped container; for a running one, use the application's own backup under
  *Settings → Backup* instead.
- **The capture agent does not run on the NAS.** Use the browser extension for link capture and
  pair it with a capture token from *Settings → System*.

## Building the image yourself

```bash
scripts/build-plugins.sh          # first! see below
scripts/docker.sh build           # caps cargo at JOBS=4
scripts/docker.sh run --port 8710
```

Three things that bite:

- **Build the plugins first.** `dist/plugins/*.rdplug` is gitignored, so a fresh clone has an
  empty directory and the image ships **no bundled plugins at all** — no hoster resolvers, no
  intake parsers. The service logs `no bundled plugin packages found` at startup when that
  happened.
- **Cap the compile.** `cargo build` inside the image otherwise uses every core.
  `scripts/docker.sh` passes `--build-arg RD_BUILD_JOBS=4`; with a plain `docker build`, pass
  it yourself.
- **Name the build.** The context carries no `.git`, so the image cannot work out its own
  commit and build time for *Settings → About rDownloader*. `scripts/docker.sh` and the release
  workflow pass `--build-arg RD_BUILD_COMMIT=… --build-arg RD_BUILD_TIME=…`, worked out by
  `rd_build_stamp` in `scripts/lib/version-file.sh`; a plain `docker build` without them shows
  both as "unknown".
- **Docker Desktop on WSL.** It writes `credsStore: desktop.exe` into `~/.docker/config.json`,
  and with WSL interop disabled that helper cannot be executed — every pull fails with
  `exec format error`. `scripts/docker.sh` builds against a credential-free config to avoid it.
  With a plain `docker build`, remove the `credsStore` line or set `DOCKER_CONFIG` to a
  directory containing only `{"auths":{}}`.

For a multi-arch image:

```bash
docker buildx build --file docker/Dockerfile \
  --platform linux/amd64,linux/arm64 --tag rdownloader:local .
```

## Troubleshooting

| Symptom | Where to look |
| --- | --- |
| Container never becomes healthy | `docker logs rdownloader`; wait for `rDownloader listening` |
| Is the service alive? | `curl -fsS http://127.0.0.1:8710/api/v1/health` |
| Environment and paths sane? | `docker exec rdownloader rdownloader doctor` |
| No hoster resolvers | Startup log says `no bundled plugin packages found` — rebuild with plugins |
| Downloads fail with permission errors | `PUID`/`PGID` do not match the folder owner on the host |
| A storage root is badged NOT PERSISTENT | The path is not mounted — add a volume for it |
| Media downloads fail | *Settings → Tools*; try a newer yt-dlp in `/config/vendor` |
| Schedules fire at the wrong time | `TZ` is unset, so the container runs on UTC |
