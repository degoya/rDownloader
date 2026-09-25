import { onUnmounted, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { BrowserSession } from '@/api/types'

/** How often a waiting request is asked about, in milliseconds. */
export const BROWSER_SESSION_POLL_MS = 3000

/**
 * Requests for a browser's session at an account's provider (RD-120-45).
 *
 * The service holds the request and the browser extension answers it, so this only starts,
 * withdraws and watches. A request stops being watched the moment it is no longer waiting;
 * `onDelivered` runs once for each one that arrived, so the account list can show its cookies
 * and check them.
 */
export function useBrowserSessions(onDelivered: (accountId: string) => void) {
  const sessions = ref<Record<string, BrowserSession | null>>({})
  const startingId = ref<string | null>(null)
  const error = ref<string | null>(null)
  let timer: number | null = null

  async function load(accountId: string): Promise<BrowserSession | null> {
    const response = await api.GET('/api/v1/accounts/{id}/browser-session', { params: { path: { id: accountId } } })
    const session = response.data ?? null
    sessions.value = { ...sessions.value, [accountId]: session }
    return session
  }

  async function begin(accountId: string): Promise<void> {
    startingId.value = accountId
    error.value = null
    const response = await api.POST('/api/v1/accounts/{id}/browser-session', { params: { path: { id: accountId } } })
    startingId.value = null
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    sessions.value = { ...sessions.value, [accountId]: response.data }
    watch()
  }

  async function cancel(accountId: string): Promise<void> {
    await api.DELETE('/api/v1/accounts/{id}/browser-session', { params: { path: { id: accountId } } })
    sessions.value = { ...sessions.value, [accountId]: null }
  }

  /** Hides a finished request's line; the service forgets it on its own. */
  function dismiss(accountId: string): void {
    sessions.value = { ...sessions.value, [accountId]: null }
  }

  function waitingIds(): string[] {
    return Object.entries(sessions.value)
      .filter(([, session]) => session?.state === 'waiting')
      .map(([id]) => id)
  }

  function watch(): void {
    if (timer !== null) return
    timer = window.setInterval(() => {
      const waiting = waitingIds()
      if (!waiting.length) return stop()
      void Promise.all(waiting.map(async (id) => {
        const session = await load(id)
        if (session?.state === 'delivered') onDelivered(id)
      }))
    }, BROWSER_SESSION_POLL_MS)
  }

  function stop(): void {
    if (timer === null) return
    window.clearInterval(timer)
    timer = null
  }

  onUnmounted(stop)

  return { sessions, startingId, error, begin, cancel, dismiss }
}
