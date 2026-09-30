/*
 * Service worker for the installable app (RD-090-08).
 *
 * Hand-written and deliberately small. What matters here is not offline capability — a
 * download manager is useless without its server — but that installing the app never makes
 * it show something that is not true. So: the API, the event stream and the compatibility
 * adapters are never touched, and the shell is served from the cache only when the network
 * has actually failed.
 */

// v2: the shell is stored under the scope's own addresses since the base-path fix; the activate
// step below drops a v1 cache that may hold API answers stored under a reverse-proxy path.
const SHELL_CACHE = 'rdownloader-shell-v2'

/**
 * Where the app is mounted, taken from the worker's own scope: `/` at the root, `/downloads/`
 * behind a reverse proxy that puts it under a path. Every address below is relative to it, so
 * the same file serves both — absolute paths missed the shell and let `/downloads/api/…` through
 * to the cache.
 */
const SCOPE = new URL(self.registration.scope)
const inScope = path => new URL(path, SCOPE).href

const SHELL = ['./', 'index.html', 'favicon.svg', 'manifest.webmanifest'].map(inScope)
const INDEX = inScope('index.html')

/** Paths, relative to the scope, whose responses must never come from a cache. */
const LIVE_PREFIXES = ['api/', 'mcp', 'sabnzbd/']

function isLive(pathname) {
  const relative = pathname.startsWith(SCOPE.pathname)
    ? pathname.slice(SCOPE.pathname.length)
    : pathname.replace(/^\/+/, '')
  return LIVE_PREFIXES.some(prefix => relative.startsWith(prefix))
}

self.addEventListener('install', event => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then(cache => cache.addAll(SHELL))
      .then(() => self.skipWaiting())
      .catch(() => undefined)
  )
})

self.addEventListener('activate', event => {
  event.waitUntil(
    caches
      .keys()
      .then(keys =>
        Promise.all(keys.filter(key => key !== SHELL_CACHE).map(key => caches.delete(key)))
      )
      .then(() => self.clients.claim())
  )
})

self.addEventListener('fetch', event => {
  const request = event.request
  if (request.method !== 'GET') return
  const url = new URL(request.url)
  if (url.origin !== self.location.origin) return
  // Live state is fetched, never cached and never substituted. A cached queue that says a
  // download is still running long after it finished is worse than no answer at all.
  if (isLive(url.pathname)) return

  if (request.mode === 'navigate') {
    // Network first: a running service always wins over the stored shell, so an updated
    // build is picked up on the next visit rather than after a cache expires.
    event.respondWith(
      fetch(request).catch(() =>
        caches.match(INDEX).then(cached => cached ?? Response.error())
      )
    )
    return
  }

  // Static assets are content-hashed, so a cache hit is the same bytes by construction.
  event.respondWith(
    caches.match(request).then(
      cached =>
        cached
        ?? fetch(request).then(response => {
          if (response.ok && response.type === 'basic') {
            const copy = response.clone()
            void caches.open(SHELL_CACHE).then(cache => cache.put(request, copy))
          }
          return response
        })
    )
  )
})
