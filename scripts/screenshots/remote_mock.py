"""The TLS half of the screenshot demo's mock (RD-1490-13): a remote-job provider's API.

Every remote-job plugin calls its provider's fixed HTTPS host, so the remote-jobs view stayed
empty while the mock spoke plain HTTP only. This module holds what mock.py needs to answer one of
them: a throwaway CA and a server certificate for HOSTS, made by `openssl` at the mock's start
into <RD_SHOTS_WORK>/tls/ (never committed, never outside the work directory), the TLS context
mock.py wraps a proxy `CONNECT` tunnel to HOSTS in, and Real-Debrid's answers -- `torrents/*` as
plugins/realdebrid-torrents calls it, `user` and `hosts/domains` as plugins/realdebrid does.
seed.mjs `remote` trusts ca.pem through the custom CA setting and submits MAGNETS.

Nothing is stored: every job is a fixed entry of JOBS, its torrent id derived from its info hash,
so a restarted mock answers the same.
"""
import hashlib
import json
import os
import ssl
import subprocess
import tempfile
from urllib.parse import parse_qs, quote

HOSTS = ('api.real-debrid.com',)
API = '/rest/1.0'
# Plain HTTP on the mock's own address: the magnets seed.mjs `remote` submits.
MAGNETS_PATH = '/remote-jobs/magnets.json'
MB = 1024 * 1024
GB = 1024 * MB

# name, Real-Debrid's status, percent, bytes per second, size: two running, one still queued at
# the provider, one finished, one failed (no peers left).
JOBS = [
    ('Public.Domain.Film.Noir.Sampler.1080p', 'dead', 0, 0, int(2.3 * GB)),
    ('Creative.Commons.Audiobook.Library.Vol.2', 'downloaded', 100, 0, 410 * MB),
    ('Blender.Open.Projects.Collection.2026', 'queued', 0, 0, int(12.8 * GB)),
    ('Elephants.Dream.2006.Remastered.1080p', 'downloading', 23, int(3.1 * MB), int(1.2 * GB)),
    ('Sintel.2010.Directors.Cut.2160p', 'downloading', 64, int(8.4 * MB), int(4.6 * GB)),
]
# Where a finished job's files lie: the demo origin, behind the same proxy profile.
READY_LINK = 'http://files.example.org/audio/Creative-Commons-Sampler-Vol1.flac'
USER = {'id': 4711, 'username': 'demo-user', 'email': 'demo@example.invalid', 'points': 0,
        'locale': 'en', 'avatar': '', 'type': 'premium', 'premium': 30 * 86400,
        'expiration': '2027-01-31T12:00:00.000Z'}
# The multihoster's catalogue: no host the demo downloads from, so nothing is routed through it.
HOSTERS = ['hoster.example.com', 'uploads.example.net']
ADDED = '2026-11-20T09:30:00.000Z'


def info_hash(name):
    return hashlib.sha1(name.encode()).hexdigest()


def torrent_id(name):
    """Real-Debrid's ids are thirteen upper-case letters and digits."""
    return info_hash(name)[:13].upper()


def magnet(name):
    return f'magnet:?xt=urn:btih:{info_hash(name)}&dn={quote(name)}'


MAGNETS = [{'name': job[0], 'magnet': magnet(job[0])} for job in JOBS]
BY_ID = {torrent_id(job[0]): job for job in JOBS}
BY_HASH = {info_hash(job[0]): job for job in JOBS}


def torrent_info(job):
    name, status, percent, speed, size = job
    files = [{'id': 1, 'path': f'/{name}/{name}.mkv' if status != 'downloaded' else f'/{name}/{name}.flac',
              'bytes': size, 'selected': 1}]
    answer = {'id': torrent_id(name), 'filename': name, 'original_filename': name, 'hash': info_hash(name),
              'bytes': size, 'original_bytes': size, 'host': 'real-debrid.com', 'split': 2000,
              'progress': percent, 'status': status, 'added': ADDED, 'files': files,
              'links': [READY_LINK] if status == 'downloaded' else []}
    if speed:
        answer['speed'] = speed
        answer['seeders'] = 12
    return answer


def listed(job):
    full = torrent_info(job)
    return {key: full[key] for key in ('id', 'filename', 'hash', 'bytes', 'host', 'split', 'progress', 'status', 'added')}


def error(status, word, code):
    return status, {'error': word, 'error_code': code}


def answer(method, path, body, authorization):
    """(status, JSON document or None) for one request to the API host."""
    if not path.startswith(API + '/'):
        return error(404, 'unknown_ressource', 7)
    route = path[len(API):]
    # The catalogue is public; everything else carries the account's token.
    if route == '/hosts/domains' and method == 'GET':
        return 200, HOSTERS
    if not (authorization or '').startswith('Bearer ') or len(authorization) <= len('Bearer '):
        return error(401, 'bad_token', 8)
    if route == '/user' and method == 'GET':
        return 200, USER
    if route == '/torrents' and method == 'GET':
        return 200, [listed(job) for job in reversed(JOBS)]
    if route == '/torrents/addMagnet' and method == 'POST':
        address = parse_qs(body.decode('utf-8', 'replace')).get('magnet', [''])[0]
        query = parse_qs(address.partition('?')[2])
        found = BY_HASH.get(query.get('xt', [''])[0].rpartition(':')[2].lower())
        if not found:
            return error(400, 'magnet_conversion', 30)
        tid = torrent_id(found[0])
        return 201, {'id': tid, 'uri': f'https://{HOSTS[0]}{API}/torrents/info/{tid}'}
    head, _, tid = route.rpartition('/')
    job = BY_ID.get(tid)
    if job is None:
        return error(404, 'unknown_ressource', 7)
    if head == '/torrents/info' and method == 'GET':
        return 200, torrent_info(job)
    if head == '/torrents/selectFiles' and method == 'POST':
        return 204, None
    if head == '/torrents/delete' and method == 'DELETE':
        return 204, None
    return error(404, 'unknown_ressource', 7)


def _openssl(*args):
    subprocess.run(['openssl', *args], check=True, capture_output=True)


def ensure_certificates(directory):
    """ca.pem and server.pem/server.key in `directory`, made once; the paths of the three."""
    ca, ca_key = os.path.join(directory, 'ca.pem'), os.path.join(directory, 'ca.key')
    cert, key = os.path.join(directory, 'server.pem'), os.path.join(directory, 'server.key')
    if all(os.path.exists(path) for path in (ca, cert, key)):
        return ca, cert, key
    os.makedirs(directory, mode=0o700, exist_ok=True)
    curve = ('-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes')
    old = os.umask(0o077)
    try:
        _openssl('req', '-x509', '-new', *curve, '-keyout', ca_key, '-out', ca, '-days', '30',
                 '-subj', '/CN=rDownloader screenshot demo CA',
                 '-addext', 'basicConstraints=critical,CA:TRUE',
                 '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
        csr, ext = os.path.join(directory, 'server.csr'), os.path.join(directory, 'server.ext')
        _openssl('req', '-new', *curve, '-keyout', key, '-out', csr, '-subj', f'/CN={HOSTS[0]}')
        with open(ext, 'w', encoding='ascii') as out:
            out.write('subjectAltName=' + ','.join(f'DNS:{host}' for host in HOSTS) + '\n'
                      'basicConstraints=critical,CA:FALSE\n'
                      'keyUsage=critical,digitalSignature\n'
                      'extendedKeyUsage=serverAuth\n')
        _openssl('x509', '-req', '-in', csr, '-CA', ca, '-CAkey', ca_key, '-days', '30',
                 '-set_serial', str(int.from_bytes(os.urandom(8), 'big')), '-extfile', ext, '-out', cert)
        os.remove(csr)
        os.remove(ext)
    finally:
        os.umask(old)
    os.chmod(ca, 0o644)
    return ca, cert, key


def server_context(work=None):
    """The context a tunnel to HOSTS is wrapped in, or None (with the reason) when openssl failed."""
    directory = os.path.join(work, 'tls') if work else tempfile.mkdtemp(prefix='rd-shots-tls-')
    try:
        _, cert, key = ensure_certificates(directory)
    except (OSError, subprocess.CalledProcessError) as failure:
        detail = getattr(failure, 'stderr', b'') or b''
        return None, f'no TLS: {failure} {detail.decode(errors="replace").strip()}'
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    context.set_alpn_protocols(['http/1.1'])
    return context, f'TLS for {", ".join(HOSTS)} from {directory}'


class Writer:
    """An unbuffered writer over the TLS socket, as http.server's own over a plain one."""

    def __init__(self, sock):
        self._sock = sock

    def write(self, data):
        self._sock.sendall(data)
        return len(data)

    def flush(self):
        pass


def encode(document):
    return b'' if document is None else json.dumps(document).encode()
