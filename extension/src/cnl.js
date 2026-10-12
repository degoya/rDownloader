// Click'n'Load without the desktop agent (RD-1260-10): pure helpers, no browser globals.
//
// A hoster page talks to `http://127.0.0.1:9666/`, where the agent listens. Without the agent —
// rDownloader on a NAS, a machine the agent is not installed on — nothing answers there and the
// page shows no button. The extension cannot open a port, so it takes those requests in the
// browser: it answers the page's check with a resource of its own and hands the fields of an
// `add` to the service. `jk` travels as the page sent it, a string; the service evaluates it in
// the agent's sandbox, and nothing here ever runs it.

import { PICK_WAITING } from './api.js'

/** The two addresses a page reaches the agent at, and the only hosts this feature needs. */
export const CNL_ORIGINS = ['http://127.0.0.1:9666', 'http://localhost:9666']

/** The same as webRequest match patterns. */
export const CNL_URL_PATTERNS = CNL_ORIGINS.map((origin) => `${origin}/*`)

/** What the page is answered with, as the agent answers it: the extension's own resources. */
export const RESOURCES = {
  check: 'src/cnl/jdcheck.js',
  flash: 'src/cnl/flash.txt',
  done: 'src/cnl/success.txt'
}

/**
 * The largest hand-over the extension passes on, the service's own limit for the route. A page
 * that sends more is refused here rather than making the browser upload it first.
 */
export const MAX_HANDOVER_BYTES = 20 * 1024 * 1024

/** The fields a page sends, under their Click'n'Load names; anything else is dropped. */
export const FIELDS = ['crypted', 'jk', 'key', 'urls', 'url', 'passwords', 'package', 'source']

/** Codes the service refuses a hand-over with, and the message that explains each one. */
export const REFUSAL_MESSAGES = {
  'cnl.invalid_payload': 'cnlInvalidPayload',
  'cnl.jk_refused': 'cnlJkRefused',
  'cnl.no_links': 'cnlNoLinks',
  'cnl.payload_too_large': 'cnlTooLarge',
  'cnl.rate_limited': 'cnlRateLimited'
}

/**
 * What a request to the port asks for: `'check'`, `'flash'`, `'add'`, `'addcrypted2'`, or null
 * for anything else. The paths are the agent's, exactly; a path the agent does not serve is left
 * to fail as it would without the extension.
 */
export function cnlRoute(url) {
  let parsed
  try {
    parsed = new URL(String(url))
  } catch {
    return null
  }
  if (!CNL_ORIGINS.includes(parsed.origin)) return null
  switch (parsed.pathname) {
    case '/jdcheck.js':
      return 'check'
    case '/flash':
      return 'flash'
    case '/flash/add':
      return 'add'
    case '/flash/addcrypted2':
      return 'addcrypted2'
    default:
      return null
  }
}

/** The page a request came from: Chrome's `initiator`, Firefox's `originUrl` or `documentUrl`. */
export function initiatorOf(details) {
  return details?.initiator ?? details?.originUrl ?? details?.documentUrl ?? null
}

function decodeRaw(raw) {
  const parts = []
  for (const part of Array.isArray(raw) ? raw : []) {
    if (part?.bytes) parts.push(new TextDecoder().decode(part.bytes))
  }
  return parts.join('')
}

function firstValues(entries) {
  const values = {}
  for (const [name, value] of entries) {
    if (FIELDS.includes(name) && typeof value === 'string' && !(name in values)) values[name] = value
  }
  return values
}

/**
 * The Click'n'Load fields of one request: the query string, and the form body over it — the
 * browser's parsed `formData`, or a raw body read as a form. The first value of each known name
 * counts, as it does on the agent's port. `null` when the request is larger than
 * `MAX_HANDOVER_BYTES` or has no address.
 */
export function requestFields(details) {
  let query
  try {
    query = firstValues(new URL(String(details?.url)).searchParams)
  } catch {
    return null
  }
  const body = details?.requestBody
  let posted = {}
  if (body?.formData && typeof body.formData === 'object') {
    posted = firstValues(Object.entries(body.formData).map(([name, values]) => [name, Array.isArray(values) ? values[0] : values]))
  } else if (body?.raw) {
    const text = decodeRaw(body.raw)
    if (text.length > MAX_HANDOVER_BYTES) return null
    posted = firstValues(new URLSearchParams(text))
  }
  const fields = { ...query, ...posted }
  const size = Object.values(fields).reduce((total, value) => total + value.length, 0)
  return size > MAX_HANDOVER_BYTES ? null : fields
}

function httpAddress(value) {
  try {
    const parsed = new URL(String(value))
    return /^https?:$/.test(parsed.protocol) ? parsed.href : null
  } catch {
    return null
  }
}

/**
 * The body of `POST /api/v1/capture/click-and-load` for one request, or null when it carries
 * nothing to hand over.
 *
 * `jk` (or `key`, its other spelling) is copied as the string the page sent. It is never parsed,
 * never evaluated: the service reads the key out of it or runs it in its sandbox (RD-1260-10).
 * The page's own `source` labels the batch when it is a web address; the page that sent the
 * request does otherwise.
 */
export function cnlPayload(route, fields, pageUrl) {
  const body = {}
  if (route === 'addcrypted2') {
    if (!fields?.crypted) return null
    body.crypted = fields.crypted
    const jk = fields.jk ?? fields.key
    if (typeof jk === 'string') body.jk = jk
  } else if (route === 'add') {
    const urls = fields?.urls ?? fields?.url
    if (!urls) return null
    body.urls = urls
  } else {
    return null
  }
  if (fields.passwords) body.passwords = fields.passwords
  if (fields.package) body.package = fields.package
  const source = httpAddress(fields.source) ?? httpAddress(pageUrl)
  if (source) body.source = source
  return body
}

function escapeRegex(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** The rule ids this feature owns among the extension's session rules. */
export const RULE_IDS = [96661, 96662, 96663]

/**
 * Chrome's session rules for the allowed sites: the check and `/flash` answered with the
 * extension's resources, an `add` answered with `success` while the observing listener hands its
 * fields over. Empty when no site is allowed — `initiatorDomains` must not be empty, and an
 * empty list would mean every page.
 */
export function sessionRules(hosts) {
  const sites = [...new Set((hosts ?? []).filter((host) => typeof host === 'string' && host))].sort()
  if (sites.length === 0) return []
  const port = `^http://(${CNL_ORIGINS.map((origin) => escapeRegex(new URL(origin).hostname)).join('|')}):9666`
  const rule = (id, path, resource) => ({
    id,
    priority: 1,
    action: { type: 'redirect', redirect: { extensionPath: `/${resource}` } },
    condition: {
      regexFilter: `${port}${path}([?#].*)?$`,
      initiatorDomains: sites,
      resourceTypes: ['main_frame', 'sub_frame', 'script', 'xmlhttprequest', 'image', 'other']
    }
  })
  return [
    rule(RULE_IDS[0], '/jdcheck\\.js', RESOURCES.check),
    rule(RULE_IDS[1], '/flash', RESOURCES.flash),
    rule(RULE_IDS[2], '/flash/add(crypted2)?', RESOURCES.done)
  ]
}

/** The message key and substitutions that tell the person how a hand-over ended. */
export function handoverOutcome(result) {
  if (result?.ok && result.code === PICK_WAITING) return { key: 'pickWaiting', substitutions: [String(result.entries ?? 0)] }
  if (result?.ok) return { key: 'cnlSent', substitutions: [String(result.links ?? 0)] }
  if (result?.status === 401) return { key: 'cnlFailed', reasonKey: 'errorUnauthorized' }
  const reasonKey = REFUSAL_MESSAGES[result?.code]
  if (reasonKey) return { key: 'cnlFailed', reasonKey }
  return { key: 'cnlFailed', reason: result?.message ?? '' }
}
