import { onUnmounted, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { AuthFlow } from '@/api/types'

/** How often an open sign-in is asked about when no event arrives, in milliseconds. */
const AUTH_FLOW_POLL_MS = 3000

/** Whether the service is still waiting on the person or the provider. */
export function isOpenFlow(flow: AuthFlow | null | undefined): boolean {
  return flow?.state === 'waiting_for_user' || flow?.state === 'polling'
}

/**
 * Sign-in flows a plugin is running, by account (RD-090-13).
 *
 * The service drives them; this only reads state and shows it. Closing the page in the middle
 * of a sign-in therefore loses nothing — reopening it picks the flow up where it stands.
 *
 * Three rules keep the code on screen for as long as the service waits on it (RD-150-09,
 * reported from 1.5.1: the Real-Debrid code vanished after a few seconds while the sign-in
 * carried on, and "Connect" then replaced it with a new one):
 *
 * - **A read that did not answer changes nothing.** Only an answer from the service replaces
 *   what is shown. A failed poll used to write `null`, which hid the code *and* stopped the
 *   watch, since no flow was open any more — while the service went on polling the provider.
 * - **"Connect" asks the service first.** A sign-in that is still open is shown, not replaced:
 *   every `begin` makes the provider issue a new code, and the one the person may be typing
 *   right now stops working.
 * - **The bus is listened to.** Every step the service records arrives as `account.changed`
 *   for the `auth_flow`, so the status is read the moment it moves; the timer only covers a
 *   stream that is down.
 */
export function useAuthFlows(onAuthorized: (accountId: string) => void) {
  const flows = ref<Record<string, AuthFlow | null>>({})
  const connectingId = ref<string | null>(null)
  let timer: number | null = null

  function flowOf(accountId: string): AuthFlow | null {
    return flows.value[accountId] ?? null
  }

  function set(accountId: string, flow: AuthFlow | null): void {
    const before = flows.value[accountId]
    flows.value = { ...flows.value, [accountId]: flow }
    if (isOpenFlow(flow)) watch()
    // A sign-in that finishes changes the account: the key badge appears once it is stored.
    if (flow?.state === 'authorized' && isOpenFlow(before)) onAuthorized(accountId)
  }

  /** Reads one account's flow; a request that did not answer keeps what is shown. */
  async function load(accountId: string): Promise<AuthFlow | null> {
    const response = await api.GET('/api/v1/accounts/{id}/auth', { params: { path: { id: accountId } } })
    if (response.error !== undefined) return flowOf(accountId)
    set(accountId, response.data ?? null)
    return flowOf(accountId)
  }

  /**
   * Shows the running sign-in, or starts one; the address is shown, never opened. Answers why a
   * start failed, or `null`.
   */
  async function connect(accountId: string): Promise<string | null> {
    connectingId.value = accountId
    if (isOpenFlow(await load(accountId))) {
      connectingId.value = null
      return null
    }
    const response = await api.POST('/api/v1/accounts/{id}/auth/begin', {
      params: { path: { id: accountId } }
    })
    connectingId.value = null
    if (!response.data) return responseError(response)
    set(accountId, response.data)
    return null
  }

  async function cancel(accountId: string): Promise<void> {
    await api.DELETE('/api/v1/accounts/{id}/auth', { params: { path: { id: accountId } } })
    flows.value = { ...flows.value, [accountId]: null }
  }

  /** `account.changed` from the bus: a flow's step is read at once, anything else is not ours. */
  function onAccountEvent(event: MessageEvent<string>): void {
    let payload: { entity?: string, account_id?: string } | undefined
    try {
      payload = (JSON.parse(event.data) as { payload?: typeof payload }).payload
    } catch {
      return
    }
    if (payload?.entity === 'auth_flow' && payload.account_id) void load(payload.account_id)
  }

  function openIds(): string[] {
    return Object.entries(flows.value)
      .filter(([, flow]) => isOpenFlow(flow))
      .map(([id]) => id)
  }

  function watch(): void {
    if (timer !== null) return
    timer = window.setInterval(() => {
      const open = openIds()
      if (!open.length) return stop()
      void Promise.all(open.map(load))
    }, AUTH_FLOW_POLL_MS)
  }

  function stop(): void {
    if (timer === null) return
    window.clearInterval(timer)
    timer = null
  }

  onUnmounted(stop)

  return { flows, connectingId, flowOf, load, connect, cancel, onAccountEvent }
}
