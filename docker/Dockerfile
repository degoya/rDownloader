FROM node:24-bookworm-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.98-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY plugins/ plugins/
COPY resources/ resources/
COPY --from=web /src/web/dist web/dist
# Empty by default so CI keeps every core. Unbounded cargo parallelism has exhausted memory on
# developer machines more than once, so a local build passes --build-arg CARGO_BUILD_JOBS=4.
ARG CARGO_BUILD_JOBS=
# The commit and build time the About page and VERSION.txt name (RD-130-12). The context has no
# .git, so without them the binary says "unknown"; scripts/docker.sh and the release workflow
# pass what `rd_build_stamp` in scripts/lib/version-file.sh works out. As build arguments they
# are in the environment of the RUN below, where crates/rdownloader/build.rs reads them.
ARG RD_BUILD_COMMIT=
ARG RD_BUILD_TIME=
RUN cargo build --locked --release ${CARGO_BUILD_JOBS:+-j ${CARGO_BUILD_JOBS}} -p rdownloader

FROM debian:bookworm-slim AS runtime
# Pinned so the image is reproducible. yt-dlp in particular ages fast; the running versions are
# visible under Settings -> External tools, and a newer one can be dropped into /config/vendor.
ARG YT_DLP_VERSION=2026.8.19
ARG STREAMLINK_VERSION=8.5.0
ARG GALLERY_DL_VERSION=1.32.11
ARG APPRISE_VERSION=1.13.1
RUN apt-get update \
    && apt-get install --no-install-recommends -y \
        7zip \
        ca-certificates \
        curl \
        ffmpeg \
        gosu \
        par2 \
        python3 \
        python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir \
        "yt-dlp==${YT_DLP_VERSION}" \
        "streamlink==${STREAMLINK_VERSION}" \
        "gallery-dl==${GALLERY_DL_VERSION}" \
        "apprise==${APPRISE_VERSION}" \
    && ln -s /opt/venv/bin/yt-dlp /opt/venv/bin/streamlink /opt/venv/bin/gallery-dl \
        /opt/venv/bin/apprise /usr/local/bin/ \
    # Post-processing and the tool status look up a binary called `7z`; Debian's 7zip package
    # installs it as `7zz`. unrar stays out on purpose: it is non-free, and unrar-free cannot
    # read RAR5, which 7zz can.
    && ln -s /usr/bin/7zz /usr/local/bin/7z \
    && useradd --create-home --uid 10001 rdownloader \
    && mkdir -p /config /downloads \
    && chown -R rdownloader:rdownloader /config /downloads
COPY --from=builder /src/target/release/rdownloader /usr/local/bin/rdownloader
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
# Signed plugin packages produced by the release workflow. A local build without
# scripts/build-plugins.sh copies an empty directory, and the service says so at startup.
COPY dist/plugins /usr/local/lib/rdownloader/plugins
VOLUME ["/config", "/downloads"]
EXPOSE 8710
ENV RDOWNLOADER_DATABASE=/config/rdownloader.sqlite3 \
    RDOWNLOADER_DOWNLOADS=/downloads \
    RDOWNLOADER_PLUGIN_ROOT=/config/plugins \
    RDOWNLOADER_BUNDLED_PLUGINS=/usr/local/lib/rdownloader/plugins \
    RDOWNLOADER_LISTEN=0.0.0.0:8710 \
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
