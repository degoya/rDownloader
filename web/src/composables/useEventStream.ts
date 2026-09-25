/**
 * One shared `EventSource` for the whole app.
 *
 * Every store used to open its own connection to `/api/v1/events`. Browsers cap HTTP/1.1 at
 * ~6 connections per host, so three permanently parked streams left only three slots for all
 * other traffic — under rapid route changes the remaining slots filled with pending GETs and
 * writes such as `PUT /api/v1/settings` never got a turn. One stream with a fan-out keeps the
 * pool free, and it is also the only place that owns the reconnect timer, so a pending retry
 * can no longer resurrect a stream after the last subscriber went away.
 */

import { withBase } from '@/basePath'

type StreamListener = (event: MessageEvent) => void

const ENDPOINT = withBase('/api/v1/events')
const BASE_RECONNECT_MS = 1_000
const MAX_RECONNECT_MS = 30_000

/**
 * The marker the server sends in place of the events a subscriber was too slow to read.
 *
 * It is not a domain event and carries no SSE `id` — an id would move the reconnect cursor past
 * the very events that were missed. It says one thing: everything this connection carries may be
 * out of date. So it is handled here rather than in each store's `subscribeEvents` map; nine
 * stores each remembering to opt in is how one of them ends up not doing it.
 */
const LAGGED_EVENT = 'stream.lagged'

/**
 * The marker the server sends when the id this connection resumed from is no longer in its
 * buffer — it fell out, or the service restarted and knows no id at all. Same consequence as the
 * lag marker: everything since the last event is gone, so every subscriber re-reads.
 */
const EXPIRED_EVENT = 'stream.expired'

/** Event name -> the subscribers interested in it. A name may have several (e.g. `usenet.changed`). */
const listeners = new Map<string, Set<StreamListener>>()
/** Names already bound on the live `EventSource`; reset on every reconnect. */
const bound = new Set<string>()

let source: EventSource | null = null
let reconnectTimer: number | null = null
let attempt = 0

function dispatch(name: string, event: Event): void {
  const subscribers = listeners.get(name)
  if (!subscribers) return
  // Copy first: a handler may unsubscribe while we iterate.
  for (const listener of [...subscribers]) listener(event as MessageEvent)
}

/**
 * Hands the lag marker to every subscriber, once each, whatever names they registered.
 *
 * The stores already answer an event of a kind they cannot read by re-reading the API — the
 * queue and the LinkGrabber debounce a `refresh()`, the captcha store refetches an envelope
 * whose list is missing — so the marker reaching them *is* the refresh, and no store needs a
 * second code path for it. A handler that only patches one figure out of a payload (torrent
 * counters, post-processing progress) finds nothing to patch and returns; its own store's
 * refreshing handler has already been called.
 *
 * Deduplicated because one handler is commonly registered under several names: the queue binds
 * the same `scheduleRefresh` to four of them, and a marker must not cost four dispatches.
 */
function dispatchLagged(event: Event): void {
  const seen = new Set<StreamListener>()
  // Copy first: a handler may unsubscribe while we iterate.
  for (const subscribers of [...listeners.values()]) {
    for (const listener of [...subscribers]) {
      if (seen.has(listener)) continue
      seen.add(listener)
      listener(event as MessageEvent)
    }
  }
}

function bind(name: string): void {
  if (!source || bound.has(name)) return
  source.addEventListener(name, event => dispatch(name, event))
  bound.add(name)
}

function clearReconnect(): void {
  if (reconnectTimer === null) return
  window.clearTimeout(reconnectTimer)
  reconnectTimer = null
}

function open(): void {
  if (source || listeners.size === 0) return
  const stream = new EventSource(ENDPOINT, { withCredentials: true })
  source = stream
  bound.clear()
  // Bound before the subscribers' own names and marked as bound, so a store that also lists
  // the marker cannot register it a second time.
  stream.addEventListener(LAGGED_EVENT, event => dispatchLagged(event))
  bound.add(LAGGED_EVENT)
  stream.addEventListener(EXPIRED_EVENT, event => dispatchLagged(event))
  bound.add(EXPIRED_EVENT)
  for (const name of listeners.keys()) bind(name)
  stream.onopen = () => { attempt = 0 }
  stream.onerror = () => {
    // Only tear down if this is still the current stream; a late error from a replaced one
    // must not close its successor.
    if (source !== stream) {
      stream.close()
      return
    }
    // While the browser is still reconnecting on its own, it is the one doing the resume: it
    // sends `Last-Event-ID` and waits the `retry:` the service asked for, and only an
    // `EventSource` that reconnects by itself carries the id — a new one starts from nothing.
    // Closing here used to throw the id away on every error. The manual reconnect below is
    // for a stream the browser gave up on: a 401 after the session ended, a wrong content type.
    if (stream.readyState !== EventSource.CLOSED) return
    source = null
    bound.clear()
    if (listeners.size === 0) return
    // Exponential backoff: a server that keeps refusing should not be hammered once a second.
    const delay = Math.min(BASE_RECONNECT_MS * 2 ** attempt, MAX_RECONNECT_MS)
    attempt += 1
    clearReconnect()
    reconnectTimer = window.setTimeout(() => { reconnectTimer = null; open() }, delay)
  }
}

function closeIfIdle(): void {
  if (listeners.size > 0) return
  clearReconnect()
  attempt = 0
  source?.close()
  source = null
  bound.clear()
}

/**
 * Subscribes to the named server events and returns the matching unsubscribe. The connection is
 * opened on the first subscription and closed once the last one is released.
 */
export function subscribeEvents(handlers: Record<string, StreamListener>): () => void {
  for (const [name, handler] of Object.entries(handlers)) {
    const subscribers = listeners.get(name) ?? new Set<StreamListener>()
    subscribers.add(handler)
    listeners.set(name, subscribers)
    bind(name)
  }
  open()

  let released = false
  return () => {
    if (released) return
    released = true
    for (const [name, handler] of Object.entries(handlers)) {
      const subscribers = listeners.get(name)
      if (!subscribers) continue
      subscribers.delete(handler)
      if (subscribers.size === 0) listeners.delete(name)
    }
    closeIfIdle()
  }
}

/** Test seam: drops every subscription and closes the stream. */
export function resetEventStream(): void {
  listeners.clear()
  closeIfIdle()
}
