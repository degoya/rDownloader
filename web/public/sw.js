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
// The build's version follows it: the page registers `sw.js?v=<version>`, so every version is a
// new worker with a cache of its own, and its activate step drops the last version's shell and
// assets instead of letting one cache grow with every update (RD-1120-16).
const BUILD = new URL(self.location.href).searchParams.get('v')
const SHELL_CACHE = BUILD ? `rdownloader-shell-v2-${BUILD}` : 'rdownloader-shell-v2'

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

/*
 * Web Push (RD-1240-13). The service encrypts each message for this browser; the push service
 * wakes the worker with it even when no tab is open. A message names its event, and a click on
 * the notification opens the view that event belongs to — in a tab of the app that is already
 * open, or in a new one.
 */

/** The view each notification event opens, relative to the scope; anything else opens the app. */
const EVENT_VIEWS = {
  package_completed: 'downloads',
  package_failed: 'downloads',
  usenet_job_hopeless: 'downloads',
  stop_mark_reached: 'downloads',
  captcha_waiting: 'downloads',
  power_pending: 'downloads',
  budget_exhausted: 'settings/bandwidth',
  storage_blocked: 'settings/routing',
  backup_failed: 'settings/backup?tab=full',
  backup_verify_failed: 'settings/backup?tab=full',
  update_available: 'settings/system?tab=updates',
  update_installed: 'settings/system?tab=updates',
  update_failed: 'settings/system?tab=updates',
  service_restarting: 'settings/system?tab=updates',
  plugin_update_available: 'settings/plugins?tab=updates',
  plugin_update_failed: 'settings/plugins?tab=updates',
  account_expiring: 'settings/accounts',
  account_invalid: 'settings/accounts',
  usenet_quota_reached: 'settings/usenet'
}

function viewFor(event) {
  return inScope(Object.hasOwn(EVENT_VIEWS, event) ? EVENT_VIEWS[event] : './')
}

function readPush(data) {
  try {
    const message = data?.json()
    if (message && typeof message === 'object') return message
  } catch {
    // Not JSON: shown as its text below.
  }
  return { body: data?.text() ?? '' }
}

self.addEventListener('push', event => {
  const message = readPush(event.data)
  const title = typeof message.title === 'string' && message.title ? message.title : 'rDownloader'
  event.waitUntil(
    self.registration.showNotification(title, {
      body: typeof message.body === 'string' ? message.body : '',
      // A delivery the service tries again replaces its first notification instead of adding one.
      tag: typeof message.tag === 'string' && message.tag ? message.tag : undefined,
      icon: inScope('icons/icon-192.png'),
      data: { url: viewFor(message.event) }
    })
  )
})

self.addEventListener('notificationclick', event => {
  event.notification.close()
  const url = event.notification.data?.url ?? inScope('./')
  event.waitUntil(
    self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then(windows => {
      const open = windows.find(client => client.url.startsWith(SCOPE.href))
      if (!open) return self.clients.openWindow(url)
      return open.focus().then(client => (client ?? open).navigate?.(url)).catch(() => undefined)
    })
  )
})
