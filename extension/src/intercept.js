// Pure helpers for browser-download interception (no browser globals — unit-testable).

/** Only these request headers are forwarded; mirrors the server-side allowlist. */
export const HEADER_ALLOWLIST = ['accept', 'accept-language', 'content-type', 'origin', 'x-requested-with']

/**
 * Highest capture contract this build speaks; the server announces its own via /capture/ping.
 *
 * A server that announces more is answered with what this build actually produces, never with a
 * claim to a shape it does not know (`captureVersion` in `downloads.js` clamps to this).
 */
export const CONTRACT_VERSION = 2

/** Backstop against credential-bearing headers even if the allowlist ever grows. */
export const HEADER_DENYLIST = /^(cookie|set-cookie|authorization|proxy-authorization|www-authenticate)$|token|secret|session|auth|key/i

/**
 * Most headers a request may contribute. Reachable, despite carrying only five allowed names:
 * a request may repeat a header, and forty `Accept` lines are forty entries.
 */
export const MAX_HEADERS = 32

const MAX_NAME = 128
const MAX_VALUE = 4096
const HTTP_SCHEME = /^https?:$/

/**
 * Lowercases, allowlists and caps request headers; oversize or credential-bearing ones are dropped.
 *
 * The denylist runs *before* the allowlist and not after it. Behind the allowlist it was a net
 * hung behind a closed door — no name that gets that far can match it, which is what
 * `replay.test.mjs` asserts — and the whole point of a backstop is to catch the day somebody
 * adds `x-auth-token` to the allowlist without thinking (RD-109-25).
 */
export function filterHeaders(headers) {
  const result = []
  for (const header of Array.isArray(headers) ? headers : []) {
    const name = String(header?.name ?? '').trim().toLowerCase()
    const value = String(header?.value ?? '')
    if (!name || HEADER_DENYLIST.test(name)) continue
    if (!HEADER_ALLOWLIST.includes(name)) continue
    if (name.length > MAX_NAME || value.length > MAX_VALUE) continue
    result.push({ name, value })
    if (result.length >= MAX_HEADERS) break
  }
  return result
}

function originOf(url) {
  try {
    return new URL(String(url)).origin
  } catch {
    return null
  }
}

function schemeOf(url) {
  try {
    return new URL(String(url)).protocol
  } catch {
    return null
  }
}

/**
 * Host match pattern for one address, or null when it has none.
 *
 * Deliberately built from `hostname` and not from `host`: a match pattern's host may not carry
 * a port, and a pattern a browser refuses is worse than no pattern at all.
 */
export function originMatchPattern(url) {
  try {
    const parsed = new URL(String(url))
    if (!HTTP_SCHEME.test(parsed.protocol)) return null
    return `${parsed.protocol}//${parsed.hostname}/*`
  } catch {
    return null
  }
}

/** Basename of a browser download path, with directories and query junk stripped. */
export function baseName(filename) {
  const text = String(filename ?? '').split(/[?#]/)[0]
  const parts = text.split(/[\\/]/)
  return parts[parts.length - 1] ?? ''
}

/** True when a browser download should be handed over to rDownloader instead of the browser. */
export function shouldIntercept(item, { enabled, serverOrigin, ownExtensionId } = {}) {
  if (!enabled || !item?.url) return false
  // Downloads we started ourselves would loop straight back into the LinkGrabber.
  if (ownExtensionId && item.byExtensionId === ownExtensionId) return false
  if (!HTTP_SCHEME.test(schemeOf(item.url) ?? '')) return false
  if (serverOrigin) {
    for (const candidate of [item.url, item.finalUrl, item.referrer]) {
      if (candidate && originOf(candidate) === serverOrigin) return false
    }
  }
  return true
}

/** Content types of files rDownloader re-fetching by address cannot be trusted with (RD-120-63). */
export const BROWSER_ONLY_TYPES = ['application/x-nzb', 'application/x-bittorrent', 'application/zip', 'application/x-zip-compressed']

/** File extensions of the same files, for a download whose type the browser did not report. */
export const BROWSER_ONLY_EXTENSIONS = ['.nzb', '.torrent', '.zip']

function hasBrowserOnlyExtension(name) {
  const lower = String(name ?? '').trim().toLowerCase()
  return lower !== '' && BROWSER_ONLY_EXTENSIONS.some((extension) => lower.endsWith(extension))
}

function pathOf(url) {
  try {
    return decodeURIComponent(new URL(String(url)).pathname)
  } catch {
    return ''
  }
}

/** Every `filename` / `filename*` a Content-Disposition names, unquoted and without charset. */
function dispositionNames(disposition) {
  const names = []
  for (const match of String(disposition ?? '').matchAll(/filename\*?\s*=\s*("[^"]*"|[^;]*)/gi)) {
    let value = match[1].trim().replace(/^"|"$/g, '')
    value = value.replace(/^[\w-]*'[\w-]*'/, '')
    try {
      value = decodeURIComponent(value)
    } catch {
      // keep the raw value
    }
    names.push(value)
  }
  return names
}

/**
 * Why a download stays with the browser instead of going to rDownloader, or null when it may go.
 *
 * - `'file'`: an NZB, torrent or ZIP, by `item.mime` or by the extension of the file name, the
 *   address or the observed Content-Disposition. rDownloader receives only the address and fetches
 *   it again without the browser's session, and the indexer carts these usually come from answer
 *   such a fetch with an error; a one-time link is already spent by then (RD-120-63). The browser
 *   finishes them, and a hotfolder on its download folder takes them into rDownloader.
 * - `'post'`: the observed request was a POST, which cannot be repeated without its body
 *   (RD-109-20). Only an *observed* POST counts: without host access for the address the
 *   extension sees no request at all and cannot tell a POST from a GET, so such a download is
 *   still handed over as before.
 */
export function staysInBrowser(item, captured = null) {
  const mime = String(item?.mime ?? '').split(';')[0].trim().toLowerCase()
  if (BROWSER_ONLY_TYPES.includes(mime)) return 'file'
  const names = [baseName(item?.filename), pathOf(item?.url), pathOf(item?.finalUrl), ...dispositionNames(captured?.contentDisposition)]
  if (names.some(hasBrowserOnlyExtension)) return 'file'
  if (String(captured?.method ?? 'GET').toUpperCase() === 'POST') return 'post'
  return null
}

/** Newest buffered request whose url matches the download, within `windowMs`; null when none. */
export function correlateRequest(pendingRequests, item, now, windowMs = 10_000) {
  const urls = new Set([item?.finalUrl, item?.url].filter(Boolean).map(String))
  let best = null
  for (const entry of Array.isArray(pendingRequests) ? pendingRequests : []) {
    if (!entry?.url || !urls.has(String(entry.url))) continue
    if (!(now - entry.timestamp >= 0) || now - entry.timestamp > windowMs) continue
    if (!best || entry.timestamp > best.timestamp) best = entry
  }
  return best
}

function browserLabel(hint, userAgent) {
  if (hint === 'Chrome' || hint === 'Firefox') return hint
  return /firefox/i.test(String(userAgent ?? '')) ? 'Firefox' : 'Chrome'
}

/**
 * Intake body for one intercepted download.
 *
 * Always a `GET`. The extension used to build a second, v2-shaped payload that carried the
 * request body of a form-triggered download; that path was removed with RD-109-20, so a
 * download the browser started with a `POST` is kept by the browser instead of being handed
 * over. The server still accepts a body under capture contract v2 — the extension no longer
 * produces one.
 */
export function buildIntakePayload({ item, captured, userAgent, browser } = {}) {
  const name = baseName(item?.filename)
  const request = { method: 'GET' }
  // effective_url is only meaningful when redirects moved the download elsewhere.
  if (item?.finalUrl && item.finalUrl !== item.url) request.effective_url = item.finalUrl
  if (item?.referrer) request.referrer = item.referrer
  if (userAgent) request.user_agent = userAgent
  if (captured?.contentDisposition) request.content_disposition = captured.contentDisposition
  if (captured) {
    const headers = filterHeaders(captured.headers)
    if (headers.length) request.headers = headers
  }
  const link = { url: item?.url }
  if (name) link.file_name = name
  link.request = request
  const payload = { source: 'browser_download', source_label: browserLabel(browser, userAgent) }
  if (name) payload.package_name = name
  payload.links = [link]
  return payload
}

/** Text-only body for servers that predate the capture contract. */
export function buildLegacyPayload(item) {
  const name = baseName(item?.filename)
  const payload = { text: String(item?.url ?? ''), source: 'browser_extension', source_label: 'Browser' }
  if (name) payload.package_name = name
  return payload
}
