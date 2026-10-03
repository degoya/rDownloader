/**
 * Whether the service can be reached right now — the one place the interface learns it.
 *
 * The sidebar used to show a green dot and "Service connected" as a fixed label, so a stopped
 * service looked exactly like a running one until an action failed. Two sources report here:
 * the shared event stream (`useEventStream`) and every API request (`api/client`). A request that
 * never got an answer, or a stream error, says the service is gone; an answer of any status, or
 * a stream that opened, says it is back.
 *
 * A loss only counts after `GRACE_MS`: the browser's own stream reconnect after a service restart
 * fires an error and reopens within a second, and that blip is not worth a red dot and a toast.
 */

import { getCurrentScope, onScopeDispose, readonly, ref, type Ref } from 'vue'

import { translateServerMessage } from '@/i18n/server'

export type ConnectionState = 'connected' | 'disconnected'

/** How long a reported loss has to stand before the interface shows it. */
export const GRACE_MS = 1_500

/** The code `api/client` answers a request with that never reached the service. */
const UNREACHABLE_CODE = 'network.unreachable'

const state = ref<ConnectionState>('connected')
/** A view whose code chunk failed to load; it is loaded again once the service is back. */
const failedRoute = ref<string | null>(null)
const reconnectListeners = new Set<() => void>()
let lossTimer: number | null = null

/** The current state, read-only; `ControlRoomLayout` turns it into the dot and the toast. */
export const serviceConnection: Readonly<Ref<ConnectionState>> = readonly(state)
/** The route whose view could not be loaded, or `null`. */
export const failedViewRoute: Readonly<Ref<string | null>> = readonly(failedRoute)

function clearLossTimer(): void {
  if (lossTimer === null) return
  window.clearTimeout(lossTimer)
  lossTimer = null
}

/** The service answered — a response of any status, or the event stream opened. */
export function reportServiceReachable(): void {
  clearLossTimer()
  if (state.value === 'connected') return
  state.value = 'connected'
  // Copy first: a listener may unregister while we iterate.
  for (const listener of [...reconnectListeners]) listener()
}

/** A request got no answer, or the event stream broke off. Shown once it stands `GRACE_MS`. */
export function reportServiceUnreachable(): void {
  if (state.value === 'disconnected' || lossTimer !== null) return
  lossTimer = window.setTimeout(() => {
    lossTimer = null
    state.value = 'disconnected'
  }, GRACE_MS)
}

/**
 * Forgets a pending or shown loss without announcing a reconnect — the event stream was closed on
 * purpose (sign-out, an ended session), and a closed stream says nothing about the service.
 */
export function resetServiceConnection(): void {
  clearLossTimer()
  state.value = 'connected'
  failedRoute.value = null
}

/** Calls `listener` every time the service is reachable again after a shown loss. */
export function onServiceReconnected(listener: () => void): () => void {
  reconnectListeners.add(listener)
  return () => reconnectListeners.delete(listener)
}

/** A lazy view chunk failed to load for `route`; remembered until the service is back. */
export function reportViewLoadFailure(route: string): void {
  failedRoute.value = route
}

/** Takes the remembered route, so it is loaded once. */
export function takeFailedViewRoute(): string | null {
  const route = failedRoute.value
  failedRoute.value = null
  return route
}

/**
 * Clears `error` once the service is back, if what it holds is the "service could not be
 * reached" refusal: that alert describes a state that has ended, and it stood on the page after
 * the service had long returned. Any other message stays until the user or the next action
 * replaces it. Stops watching with the store or component it was called in.
 */
export function clearWhenReconnected(error: Ref<string | null>): () => void {
  const stop = onServiceReconnected(() => {
    const unreachable = translateServerMessage({ code: UNREACHABLE_CODE })
    if (error.value?.includes(unreachable)) error.value = null
  })
  if (getCurrentScope()) onScopeDispose(stop)
  return stop
}

/** Whether a router error is a code chunk that could not be fetched (each browser words it). */
export function isChunkLoadError(error: unknown): boolean {
  if (!(error instanceof Error)) return false
  // Chromium, Firefox and WebKit, in that order.
  return /dynamically imported module|error loading dynamically|module script failed/i.test(error.message)
}

/** Test seam: back to a fresh, connected state with no listeners. */
export function resetServiceConnectionForTests(): void {
  resetServiceConnection()
  reconnectListeners.clear()
}
