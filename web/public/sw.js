/*
 * Service worker for the installable app (RD-090-08).
 *
 * Hand-written and deliberately small. What matters here is not offline capability — a
 * download manager is useless without its server — but that installing the app never makes
 * it show something that is not true. So: the API, the event stream and the compatibility
 * adapters are never touched, and the shell is served from the cache only when the network
 * has actually failed.
 */

const SHELL_CACHE = 'rdownloader-shell-v1'
const SHELL = ['/', '/index.html', '/favicon.svg', '/manifest.webmanifest']

/** Paths whose responses must never come from a cache. */
const LIVE_PREFIXES = ['/api/', '/mcp', '/sabnzbd/', '/api/v2/']

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
  if (LIVE_PREFIXES.some(prefix => url.pathname.startsWith(prefix))) return

  if (request.mode === 'navigate') {
    // Network first: a running service always wins over the stored shell, so an updated
    // build is picked up on the next visit rather than after a cache expires.
    event.respondWith(
      fetch(request).catch(() =>
        caches.match('/index.html').then(cached => cached ?? Response.error())
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
