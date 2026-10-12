#!/usr/bin/env python3
"""Mock origin for the screenshot demo.

Serves as a plain HTTP server *and* as a forward proxy (absolute-URI requests), so rDownloader
can be pointed at it through a proxy profile and the queue shows neutral example.* hostnames.
Content is synthetic (repeating bytes); sizes come from CATALOG. Range requests are honoured.
Also answers a tiny Newznab API (t=caps / t=search) and an RSS feed. The music search holds three
more hits once <RD_SHOTS_WORK>/more-music exists (the `held` view). On a second port the news
server of nntp_mock.py, whose NZB this serves at nntp_mock.NZB_PATH (the `usenet` group).
Usage: mock.py <port> [<nntp-port>] (the NNTP port defaults to RD_SHOTS_NNTP_PORT, else 18792)
"""
import os
import hashlib
import sys
import threading
import time
import zlib
import struct
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs
from email.utils import formatdate

sys.dont_write_bytecode = True  # no __pycache__ in the tree for the module beside this file
import nntp_mock  # noqa: E402

MB = 1024 * 1024
GB = 1024 * MB

# path -> size in bytes
CATALOG = {
    # completed quickly
    '/open-movies/Big.Buck.Bunny.2008.1080p.mp4': 38 * MB,
    '/open-movies/Elephants.Dream.2006.1080p.mp4': 27 * MB,
    '/open-movies/Caminandes.Llamigos.2016.1080p.mp4': 14 * MB,
    '/audio/Creative-Commons-Sampler-Vol1.flac': 22 * MB,
    '/docs/Open-Data-Handbook-2026.pdf': 6 * MB,
    '/datasets/city-weather-2025.parquet': 31 * MB,
    # long running (throttled)
    '/open-movies/Sintel.2010.2160p.mkv': int(4.2 * GB),
    '/open-movies/Tears.of.Steel.2012.2160p.mkv': int(6.3 * GB),
    '/open-movies/Cosmos.Laundromat.2015.1080p.mkv': int(2.7 * GB),
    '/open-movies/Spring.2019.1080p.mkv': int(1.9 * GB),
    '/open-movies/Sprite.Fright.2021.1080p.mkv': int(3.1 * GB),
    '/open-movies/Charge.2022.1080p.mkv': int(2.2 * GB),
    '/open-movies/Wing.It.2023.1080p.mkv': int(1.6 * GB),
    '/podcasts/open-source-hour-ep-112.mp3': 64 * MB,
    '/podcasts/open-source-hour-ep-113.mp3': 71 * MB,
    '/podcasts/open-source-hour-ep-114.mp3': 58 * MB,
    '/linux/community-distro-26.04-desktop-amd64.iso': int(5.4 * GB),
    '/linux/community-distro-26.04-server-amd64.iso': int(2.6 * GB),
    '/assets/nature-footage-4k-pack.part1.rar': int(1.5 * GB),
    '/assets/nature-footage-4k-pack.part2.rar': int(1.5 * GB),
    '/assets/nature-footage-4k-pack.part3.rar': int(0.8 * GB),
    '/fonts/open-font-collection-2026.zip': 180 * MB,
    '/textures/pbr-texture-library-v3.7z': int(2.4 * GB),
    '/talks/open-source-summit-2026-keynote.mp4': 820 * MB,
    '/talks/open-source-summit-2026-panel.mp4': 640 * MB,
    '/nzb/sintel.nzb': 0,  # generated
}

NEWZNAB_ITEMS = [
    ('Open.Movie.Project.Sintel.2010.2160p.HDR.x265-OMP', 'Movies > UHD', 2040, 4.1, 'Animation, Fantasy', 'Sintel', '7.4'),
    ('Big.Buck.Bunny.2008.1080p.BluRay.x264-OMP', 'Movies > HD', 2040, 0.9, 'Animation, Comedy', 'Big Buck Bunny', '6.5'),
    ('Tears.of.Steel.2012.2160p.WEB.x265-OMP', 'Movies > UHD', 2045, 6.2, 'Sci-Fi, Short', 'Tears of Steel', '6.1'),
    ('Cosmos.Laundromat.2015.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 2.6, 'Animation, Drama', 'Cosmos Laundromat', '7.0'),
    ('Elephants.Dream.2006.1080p.Remastered.x264-OMP', 'Movies > HD', 2040, 1.1, 'Animation, Sci-Fi', 'Elephants Dream', '6.3'),
    ('Spring.2019.2160p.WEB.x265-OMP', 'Movies > UHD', 2045, 1.8, 'Animation, Fantasy', 'Spring', '7.6'),
    ('Sprite.Fright.2021.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 3.0, 'Animation, Horror', 'Sprite Fright', '7.1'),
    ('Charge.2022.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 2.1, 'Animation, Action', 'Charge', '6.8'),
    ('Wing.It.2023.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 1.5, 'Animation, Comedy', 'Wing It', '7.3'),
    ('Agent.327.Operation.Barbershop.2017.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 0.7, 'Animation, Action', 'Agent 327', '7.9'),
    ('Caminandes.Llamigos.2016.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 0.3, 'Animation, Comedy', 'Caminandes', '7.2'),
    ('Glass.Half.2015.1080p.WEB.h264-OMP', 'Movies > HD', 2040, 0.4, 'Animation, Comedy', 'Glass Half', '6.4'),
]

MUSIC_ITEMS = [
    ('Creative.Commons.Sampler.Vol.1-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.42, 'Electronic', 'CC Sampler 1'),
    ('Creative.Commons.Sampler.Vol.2-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.39, 'Ambient', 'CC Sampler 2'),
    ('Public.Domain.Piano.Works-FLAC-2025-CCM', 'Audio > Lossless', 3040, 0.51, 'Classical', 'Piano Works'),
    ('Open.Orchestra.Live.Session-FLAC-2025-CCM', 'Audio > Lossless', 3040, 0.63, 'Classical', 'Open Orchestra'),
    ('Field.Recordings.Forest.Dawn-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.28, 'Soundscape', 'Forest Dawn'),
    ('Chiptune.Collective.Summer-MP3-320-2026-CCM', 'Audio > MP3', 3010, 0.12, 'Chiptune', 'Chiptune Summer'),
    ('Lo-Fi.Study.Beats.Free.Edition-MP3-320-2025-CCM', 'Audio > MP3', 3010, 0.15, 'Lo-Fi', 'Lo-Fi Beats'),
    ('Jazz.Commons.Late.Night-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.47, 'Jazz', 'Jazz Commons'),
]

MORE_MUSIC = [
    ('Open.Choir.Winter.Concert-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.55, 'Choral', 'Open Choir'),
    ('Synthwave.Commons.Night.Drive-MP3-320-2026-CCM', 'Audio > MP3', 3010, 0.14, 'Synthwave', 'Night Drive'),
    ('Folk.Archive.Session.Recordings-FLAC-2026-CCM', 'Audio > Lossless', 3040, 0.44, 'Folk', 'Folk Archive'),
]

PALETTE = [(0x1f, 0x6f, 0x8b), (0x8b, 0x3a, 0x62), (0x2e, 0x7d, 0x32), (0xc7, 0x7d, 0x1a), (0x4a, 0x4e, 0xa8),
           (0x0e, 0x8f, 0x86), (0xa8, 0x32, 0x32), (0x55, 0x5f, 0x6d), (0x6a, 0x3d, 0x9a), (0x1a, 0x8c, 0xc7)]


def cover_svg(title, idx, w, h):
    a = PALETTE[idx % len(PALETTE)]
    b = PALETTE[(idx + 3) % len(PALETTE)]
    circles = ''.join(
        f'<circle cx="{(idx * 37 + k * 91) % w}" cy="{(idx * 53 + k * 67) % h}" r="{30 + (k * 23 + idx * 7) % 90}" fill="#fff" opacity="0.{(k % 3) + 1}"/>'
        for k in range(6))
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">'
            f'<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="rgb{a}"/>'
            f'<stop offset="1" stop-color="rgb{b}"/></linearGradient></defs>'
            f'<rect width="{w}" height="{h}" fill="url(#g)"/>{circles}'
            f'<text x="{w // 2}" y="{h - 40}" font-family="Georgia, serif" font-size="{max(18, w // 12)}" fill="#fff" '
            f'text-anchor="middle" opacity="0.92">{title}</text></svg>')


def pubdate(hours_ago):
    return formatdate(time.time() - hours_ago * 3600, usegmt=True)


def newznab(query, host):
    t = query.get('t', ['search'])[0]
    if t == 'caps':
        return ('<?xml version="1.0" encoding="UTF-8"?><caps><server title="Demo Indexer"/>'
                '<limits max="100" default="50"/><searching><search available="yes" supportedParams="q"/>'
                '<tv-search available="yes"/><movie-search available="yes"/><audio-search available="yes"/></searching>'
                '<categories><category id="2000" name="Movies"><subcat id="2040" name="HD"/><subcat id="2045" name="UHD"/></category>'
                '<category id="3000" name="Audio"><subcat id="3010" name="MP3"/><subcat id="3040" name="Lossless"/></category>'
                '</categories></caps>')
    offset = int(query.get('offset', ['0'])[0])
    cat = query.get('cat', [''])[0]
    items = []
    if offset == 0:
        if cat.startswith('3'):
            music = MUSIC_ITEMS + (MORE_MUSIC if os.path.exists(os.path.join(os.environ.get('RD_SHOTS_WORK', ''), 'more-music')) else [])
            for i, (title, catname, catid, gb, genre, short) in enumerate(music):
                items.append(item_xml(host, title, catname, catid, gb, i + 20, genre, short, None, 'square', i * 5 + 2))
        else:
            for i, (title, catname, catid, gb, genre, short, score) in enumerate(NEWZNAB_ITEMS):
                items.append(item_xml(host, title, catname, catid, gb, i, genre, short, score, 'wide', i * 3 + 1))
    return ('<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">'
            f'<channel><title>Demo Indexer</title><newznab:response offset="{offset}" total="{len(items)}"/>'
            + ''.join(items) + '</channel></rss>')


def item_xml(host, title, catname, catid, gb, idx, genre, short, score, shape, hours):
    size = int(gb * GB)
    guid = hashlib.sha1(title.encode()).hexdigest()[:16]
    link = f'http://{host}/getnzb/{guid}.nzb'
    cover = f'http://covers.example.net/{shape}/{idx}.svg?t={short.replace(" ", "+")}'
    attrs = [('category', str(catid)), ('size', str(size)), ('grabs', str(40 + idx * 17)),
             ('genre', genre), ('coverurl', cover), ('language', 'English'),
             ('usenetdate', pubdate(hours)), ('group', 'alt.binaries.example')]
    if score:
        attrs.append(('imdbscore', score))
    a = ''.join(f'<newznab:attr name="{k}" value="{v.replace("&", "&amp;")}"/>' for k, v in attrs)
    return (f'<item><title>{title}</title><guid isPermaLink="false">{guid}</guid><link>{link}</link>'
            f'<pubDate>{pubdate(hours)}</pubDate><category>{catname}</category>'
            f'<enclosure url="{link}" length="{size}" type="application/x-nzb"/>{a}</item>')


def rss_feed(host):
    eps = [(114, 'Packaging for everyone', 58), (113, 'Reproducible builds in practice', 71),
           (112, 'Maintainers and burnout', 64), (111, 'A year of open hardware', 55), (110, 'Community translation', 49)]
    items = ''.join(
        f'<item><title>Open Source Hour #{n}: {name}</title><guid>osh-{n}</guid>'
        f'<link>http://{host}/podcasts/episode-{n}.html</link><pubDate>{pubdate((114 - n) * 168 + 5)}</pubDate>'
        f'<enclosure url="http://{host}/podcasts/open-source-hour-ep-{n}.mp3" length="{mb * MB}" type="audio/mpeg"/>'
        f'<itunes:duration>{50 + n % 7}:{10 + n % 40:02d}</itunes:duration></item>'
        for n, name, mb in eps)
    return ('<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">'
            f'<channel><title>Open Source Hour</title><link>http://{host}/podcasts/</link>{items}</channel></rss>')


def nzb_doc(name):
    segs = ''.join(f'<segment bytes="768000" number="{i}">demo-{name}-{i}@example.invalid</segment>' for i in range(1, 6))
    return ('<?xml version="1.0" encoding="UTF-8"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">'
            f'<file poster="demo@example.invalid" date="{int(time.time())}" subject="&quot;{name}.mkv&quot; yEnc (1/5)">'
            f'<groups><group>alt.binaries.example</group></groups><segments>{segs}</segments></file></nzb>')


CHUNK = bytes(range(256)) * 256  # 64 KiB


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, fmt, *args):
        sys.stderr.write('%s %s\n' % (self.address_string(), fmt % args))

    def _target(self):
        parts = urlsplit(self.path)
        host = parts.netloc or self.headers.get('Host', 'localhost')
        return host, parts.path, parse_qs(parts.query)

    def _text(self, body, ctype, status=200):
        data = body.encode()
        self.send_response(status)
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(data)

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        host, path, query = self._target()
        if path in ('/api', '/api/'):
            return self._text(newznab(query, host), 'application/rss+xml')
        if path == '/podcasts/feed.xml':
            return self._text(rss_feed(host), 'application/rss+xml')
        if path.startswith('/getnzb/') or path in ('/nzb/sintel.nzb', nntp_mock.NZB_PATH):
            name = path.rsplit('/', 1)[-1].removesuffix('.nzb')
            self.send_response(200)
            data = (nntp_mock.nzb_document() if path == nntp_mock.NZB_PATH else nzb_doc(name)).encode()
            self.send_header('Content-Type', 'application/x-nzb')
            self.send_header('Content-Disposition', f'attachment; filename="{name}.nzb"')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            if self.command != 'HEAD':
                self.wfile.write(data)
            return
        if path.startswith('/wide/') or path.startswith('/square/'):
            shape, rest = path.split('/')[1], path.split('/')[2]
            idx = int(rest.split('.')[0])
            title = query.get('t', [''])[0]
            w, h = (800, 400) if shape == 'wide' else (500, 500)
            return self._text(cover_svg(title, idx, w, h), 'image/svg+xml')
        if path.endswith('.html'):
            return self._text('<html><body><h1>Demo page</h1></body></html>', 'text/html')
        size = CATALOG.get(path)
        if size is None:
            return self._text('not found', 'text/plain', 404)
        start, end, status = 0, size - 1, 200
        rng = self.headers.get('Range')
        if rng and rng.startswith('bytes='):
            a, _, b = rng[6:].split(',')[0].partition('-')
            start = int(a) if a else max(0, size - int(b))
            end = int(b) if (a and b) else size - 1
            end = min(end, size - 1)
            status = 206
        length = end - start + 1
        self.send_response(status)
        ctype = {'mp4': 'video/mp4', 'mkv': 'video/x-matroska', 'mp3': 'audio/mpeg', 'flac': 'audio/flac',
                 'pdf': 'application/pdf', 'zip': 'application/zip', 'iso': 'application/x-iso9660-image',
                 'rar': 'application/vnd.rar', '7z': 'application/x-7z-compressed'}.get(path.rsplit('.', 1)[-1], 'application/octet-stream')
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(length))
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('Last-Modified', 'Tue, 01 Sep 2026 10:00:00 GMT')
        self.send_header('ETag', '"' + hashlib.md5(path.encode()).hexdigest() + '"')
        if status == 206:
            self.send_header('Content-Range', f'bytes {start}-{end}/{size}')
        self.end_headers()
        if self.command == 'HEAD':
            return
        sent = 0
        try:
            while sent < length:
                n = min(len(CHUNK), length - sent)
                self.wfile.write(CHUNK[:n])
                sent += n
        except (BrokenPipeError, ConnectionResetError):
            pass


if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18791
    nntp_port = int(sys.argv[2]) if len(sys.argv) > 2 else int(os.environ.get('RD_SHOTS_NNTP_PORT', '18792'))
    threading.Thread(target=nntp_mock.serve, args=(nntp_port,), daemon=True).start()
    ThreadingHTTPServer(('127.0.0.1', port), Handler).serve_forever()
