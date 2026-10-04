# Two ways to the same image (RD-140-25):
#
#   runtime   (the default, last stage) compiles rDownloader in the `builder` stage. For a local
#             build without a Linux binary at hand: scripts/docker.sh build.
#   prebuilt  copies the binary the release workflow already built, from
#             dist/docker/linux-$TARGETARCH/rdownloader in the build context, and compiles nothing.
#             CI and the release take this one: `--target prebuilt`.
#
# Both finish on `runtime-base`, so tools, users, volumes and health check cannot drift apart.
# BuildKit builds only the stages the chosen target needs: `prebuilt` never touches node or rust.
#
# Every base image is pinned by its tag and the digest of its multi-platform index (RD-191-09), so
# a tag moved upstream changes nothing here until Dependabot's `docker` update moves both.

FROM node:24-bookworm-slim@sha256:0e0ff40c39bc087845bfb27465a0df4ea419520094bc35842ff83dd8cbe6f9b6 AS web
WORKDIR /src/web
# pnpm in the version web/package.json pins (`packageManager`), through Corepack (RD-150-14).
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm run build

FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY plugins/ plugins/
COPY resources/ resources/
COPY --from=web /src/web/dist web/dist
# Compiled in, not served: rd-diagnostics embeds the English log catalogue
# (crates/rd-diagnostics/src/notes.rs, include_str!). Without it the image build fails.
COPY web/src/locales/ web/src/locales/
# Empty by default so CI keeps every core. Unbounded cargo parallelism has exhausted memory on
# developer machines more than once, so a local build passes --build-arg RD_BUILD_JOBS=4. Not
# named CARGO_BUILD_JOBS: a build argument is in the RUN's environment, and cargo reads that
# variable itself and refuses an empty one ("could not parse ``"), which broke the release
# image of 1.2.4 and 1.3.0.
ARG RD_BUILD_JOBS=
# The commit and build time the About page and VERSION.txt name (RD-130-12). The context has no
# .git, so without them the binary says "unknown"; scripts/docker.sh and the release workflow
# pass what `rd_build_stamp` in scripts/lib/version-file.sh works out. As build arguments they
# are in the environment of the RUN below, where crates/rdownloader/build.rs reads them.
ARG RD_BUILD_COMMIT=
ARG RD_BUILD_TIME=
RUN cargo build --locked --release ${RD_BUILD_JOBS:+-j ${RD_BUILD_JOBS}} -p rdownloader

# Debian 13, glibc 2.41. The base sets the glibc floor for the prebuilt binary: the release builds
# it on ubuntu-24.04 (glibc 2.39), and a binary linked against a newer glibc than the image has
# does not start ("GLIBC_2.xx not found"). Moving the release runners to a newer Ubuntu therefore
# means checking this line first; the `--version` guard below fails the build if it was not.
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS runtime-base
# The Python tools — yt-dlp, streamlink, gallery-dl, apprise — and every dependency, pinned with
# their hashes in docker/requirements.txt, which scripts/docker-tools.sh compiles from
# docker/requirements.in (RD-191-09). `--bump` there moves them; yt-dlp in particular ages fast.
COPY docker/requirements.txt /tmp/requirements.txt
# RAR extraction (owner, 2026-09-26): Debian builds 7-Zip without the RAR codec, which lives in
# `7zip-rar` in the non-free component, under the unRAR licence (extract only). The component is
# switched on for that one package; the sed is checked, since a changed sources file would
# otherwise just leave the package unfound.
RUN sed -i 's/^Components: main$/Components: main non-free/' /etc/apt/sources.list.d/debian.sources \
    && grep -q '^Components: main non-free$' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install --no-install-recommends -y \
        7zip \
        7zip-rar \
        ca-certificates \
        curl \
        ffmpeg \
        gosu \
        par2 \
        python3 \
        python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir --require-hashes --only-binary=:all: \
        -r /tmp/requirements.txt \
    && rm /tmp/requirements.txt \
    && ln -s /opt/venv/bin/yt-dlp /opt/venv/bin/streamlink /opt/venv/bin/gallery-dl \
        /opt/venv/bin/apprise /usr/local/bin/ \
    # Post-processing and the tool status look up a binary called `7z`. Debian 13's 7zip package
    # installs it under that name (Debian 12's had only `7zz`, which needed a link). Checked here,
    # together with the RAR5 codec from 7zip-rar (codec id 40305), because either missing would
    # otherwise surface only as a failed unpack. The codec line, not the format line: the format
    # can be listed without a codec to decode it.
    && { 7z i | grep -Eq '(^|[[:space:]])40305[[:space:]]+Rar5([[:space:]]|$)' \
        || { 7z i | sed -n '/^Codecs:/,$p'; echo "7z has no RAR5 codec" >&2; exit 1; }; } \
    && useradd --create-home --uid 10001 rdownloader \
    && mkdir -p /config /downloads \
    && chown -R rdownloader:rdownloader /config /downloads
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
# Signed plugin packages produced by the release workflow. A local build without
# scripts/build-plugins.sh copies an empty directory, and the service says so at startup.
COPY dist/plugins /usr/local/lib/rdownloader/plugins
VOLUME ["/config", "/downloads"]
EXPOSE 8710
# RDOWNLOADER_INSTALL_KIND: the update check shows `docker pull` rather than a download
# (RD-180-01), also under a runtime that leaves no /.dockerenv behind.
ENV RDOWNLOADER_DATABASE=/config/rdownloader.sqlite3 \
    RDOWNLOADER_DOWNLOADS=/downloads \
    RDOWNLOADER_PLUGIN_ROOT=/config/plugins \
    RDOWNLOADER_BUNDLED_PLUGINS=/usr/local/lib/rdownloader/plugins \
    RDOWNLOADER_LISTEN=0.0.0.0:8710 \
    RDOWNLOADER_INSTALL_KIND=docker \
    PUID=10001 \
    PGID=10001 \
    TZ=UTC
# The start period is generous on purpose: installing the bundled plugins and loading the
# resolver components takes about 25 seconds on a developer machine and longer on a NAS.
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8710/api/v1/health || exit 1
# Starts as root only to adopt PUID/PGID, then drops to that user via gosu. An explicit
# `user:` in compose skips all of it.
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["serve"]

FROM runtime-base AS prebuilt
# Set by buildx per platform: amd64 or arm64. The workflows unpack the release tarballs there.
ARG TARGETARCH
COPY --chmod=0755 dist/docker/linux-${TARGETARCH}/rdownloader /usr/local/bin/rdownloader
# The guard: a binary that is missing fails the COPY above, one that cannot run here — the wrong
# architecture, a glibc newer than the base's — fails this line, not the first start of a user.
RUN rdownloader --version

FROM runtime-base AS runtime
COPY --from=builder /src/target/release/rdownloader /usr/local/bin/rdownloader
RUN rdownloader --version
