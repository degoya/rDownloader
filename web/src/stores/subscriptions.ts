import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'
import type {
  IndexerCaps,
  Subscription,
  SubscriptionBulkStateResponse,
  SubscriptionHistoryClearResponse,
  SubscriptionItemState,
  SubscriptionRequest,
  SubscriptionRun
} from '@/api/types'

import { useSubscriptionItems } from './subscriptionsItems'

// The hit lists and review counts live in `subscriptionsItems` (RD-140-27); the filter type
// stays importable from here, where callers look for it.
export type { SubscriptionItemFilter } from './subscriptionsItems'

/** What the server said when a check finished (RD-106-09). */
export interface FinishedPoll {
  subscriptionId: string
  found: number
  accepted: number
  skipped: number
  error: string | null
}

/**
 * Subscriptions and the items they found (RD-080-07).
 *
 * Items and runs are fetched per subscription rather than eagerly for all of them: the
 * archive grows without limit and only the expanded one is ever on screen.
 */
export const useSubscriptionsStore = defineStore('subscriptions', () => {
  const subscriptions = ref<Subscription[]>([])
  const { items, itemPages, itemQueries, itemErrors, reviewSummary, loadItems, reloadItems, loadMoreItems, loadReviewSummary, noteChange } =
    useSubscriptionItems()
  const runs = ref<Record<string, SubscriptionRun[]>>({})
  const error = ref<string | null>(null)
  const busy = ref(false)
  /** True while the subscription list is being fetched — `busy` covers the write actions. */
  const fetching = ref(false)
  /** True once the first fetch has settled, so "no subscriptions" is only said when it is true. */
  const settled = ref(false)
  /**
   * What a view shows in place of an empty list while the **first** fetch is on its way
   * (RD-104-07).
   *
   * Not `fetching.value || !settled.value` (RD-106-19): `fetching` goes true on every
   * `refresh()`, and a refresh also runs on the poll timer and after every check, so an empty
   * list kept trading its empty state for the loading skeleton and back — the reported
   * flicker. `design.md` promises that surface for the first fetch alone. `settled` is set
   * even when that first fetch failed, which is what we want: `loading` turns false, and
   * `DataState` renders the error it prefers over the empty state instead. `fetching` itself is
   * untouched: it still guards the scheduled refresh, and `busy`/`pollingIds` still drive the
   * buttons.
   */
  const loading = computed(() => !settled.value)
  /**
   * The subscriptions whose "check now" request is in flight (RD-106-09).
   *
   * A set rather than one flag: several subscriptions are checked independently, and a global
   * flag would lock every other button while one of them is being asked. It is released when
   * the request answers, not when the poll finishes — the server answers first on purpose, and
   * a busy state that ends on an event which may not arrive is a button that never comes back.
   * What happens after the request is carried by the notice the view raises from `lastPoll`.
   */
  const pollingIds = ref<Set<string>>(new Set())
  /** The last check that finished, as the event stream reported it. */
  const lastPoll = ref<FinishedPoll | null>(null)

  let releaseEvents: (() => void) | null = null
  let refreshTimer: number | null = null

  async function refresh(): Promise<void> {
    fetching.value = true
    const response = await api.GET('/api/v1/subscriptions')
    fetching.value = false
    settled.value = true
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    error.value = null
    subscriptions.value = response.data
  }

  async function loadRuns(id: string): Promise<void> {
    const response = await api.GET('/api/v1/subscriptions/{id}/runs', { params: { path: { id } } })
    if (response.data) runs.value = { ...runs.value, [id]: response.data }
  }

  async function create(body: SubscriptionRequest): Promise<boolean> {
    busy.value = true
    const response = await api.POST('/api/v1/subscriptions', { body })
    busy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  async function update(id: string, body: SubscriptionRequest): Promise<boolean> {
    busy.value = true
    const response = await api.PUT('/api/v1/subscriptions/{id}', { params: { path: { id } }, body })
    busy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  async function setEnabled(id: string, enabled: boolean): Promise<void> {
    const response = enabled
      ? await api.POST('/api/v1/subscriptions/{id}/enable', { params: { path: { id } } })
      : await api.POST('/api/v1/subscriptions/{id}/disable', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  async function remove(id: string): Promise<void> {
    const response = await api.DELETE('/api/v1/subscriptions/{id}', { params: { path: { id } } })
    if (response.error) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  /**
   * Asks an indexer what it can do (RD-080-11).
   *
   * Doubles as the test action: it is the cheapest request that proves the address and the
   * API key are both right, and it pulls no results.
   */
  async function loadCaps(id: string): Promise<IndexerCaps | { error: string }> {
    const response = await api.POST('/api/v1/subscriptions/{id}/caps', { params: { path: { id } } })
    return response.data ?? { error: responseError(response) }
  }

  /**
   * Asks an indexer what it can do before there is a subscription to ask for.
   *
   * The `{id}` route resolves the key out of the vault, which is why categories could only be
   * mapped after saving and reopening. This one carries the address and the key instead; the
   * key is used for that request and not stored.
   */
  async function probeCaps(url: string, apiKey: string): Promise<IndexerCaps | { error: string }> {
    const response = await api.POST('/api/v1/subscriptions/caps', { body: { url, api_key: apiKey } })
    return response.data ?? { error: responseError(response) }
  }

  /**
   * Polls one subscription now; the server answers before the poll finishes (RD-106-09).
   *
   * The failure is returned rather than written to `error`: the view names the subscription
   * it belongs to, and "check now failed" one line above a list of nine subscriptions is not
   * a message anybody can act on. A second press while the first request is in flight is
   * refused here as well as at the button, so a double click cannot ask twice.
   */
  async function pollNow(id: string): Promise<{ ok: boolean, error: string | null }> {
    if (pollingIds.value.has(id)) return { ok: false, error: null }
    pollingIds.value = new Set([...pollingIds.value, id])
    const response = await api.POST('/api/v1/subscriptions/{id}/poll', { params: { path: { id } } })
    const next = new Set(pollingIds.value)
    next.delete(id)
    pollingIds.value = next
    if (response.error) return { ok: false, error: responseError(response) }
    return { ok: true, error: null }
  }

  /** Re-reads the list, at most once per burst of events. */
  function scheduleRefresh(): void {
    if (refreshTimer !== null) return
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null
      // Re-arm rather than stacking a second request on one already in flight.
      if (fetching.value) return scheduleRefresh()
      void refresh()
    }, 300)
  }

  /**
   * What the bus says about subscriptions (RD-106-09).
   *
   * Every write emits `subscription.changed`; the one written when a poll is recorded carries
   * `poll: "finished"` with the subscription and its counts, which is the only way the view
   * can learn that a check it started is over without asking on a timer. The list is re-read
   * always and the review counts whenever a view showed them. A hit list is re-read for its
   * cause (RD-110-30): a finished poll that created rows invalidates its subscription's list
   * here; the anonymous event invalidates through the counts it moves, in
   * `noteReportedPending`; and the stream's own markers — `stream.lagged`, `stream.expired`,
   * which carry no `payload` — say that events were lost, so every wanted list may be stale.
   * The lists themselves are `subscriptionsItems`'s; this hands it the cause in `noteChange`.
   */
  function onChanged(event: MessageEvent<string>): void {
    let payload: {
      poll?: string
      subscription_id?: string
      found?: number
      accepted?: number
      skipped?: number
      error?: string | null
    } | undefined
    try {
      payload = (JSON.parse(event.data) as { payload?: typeof payload }).payload
    } catch {
      return
    }
    let wroteRowsFor: string | null = null
    const id = payload?.subscription_id
    if (payload?.poll === 'finished' && id) {
      lastPoll.value = {
        subscriptionId: id,
        found: payload.found ?? 0,
        accepted: payload.accepted ?? 0,
        skipped: payload.skipped ?? 0,
        error: payload.error ?? null
      }
      // Only a poll that wrote rows changed the hits; `found` counts what the source offered,
      // known entries included, and a failed poll wrote nothing.
      if ((payload.accepted ?? 0) + (payload.skipped ?? 0) > 0) wroteRowsFor = id
      if (runs.value[id]) void loadRuns(id)
    }
    noteChange({ lost: !payload, wroteRowsFor })
    scheduleRefresh()
  }

  function connectEvents(): void {
    if (releaseEvents) return
    releaseEvents = subscribeEvents({ 'subscription.changed': onChanged })
  }

  function disconnectEvents(): void {
    releaseEvents?.()
    releaseEvents = null
    if (refreshTimer !== null) {
      window.clearTimeout(refreshTimer)
      refreshTimer = null
    }
  }

  /** One item's decision, without the reload; the callers below decide when to re-read. */
  async function putItemState(id: string, state: SubscriptionItemState): Promise<boolean> {
    const response = await api.PUT('/api/v1/subscriptions/items/{id}', {
      params: { path: { id } },
      body: { state }
    })
    if (response.error) {
      error.value = responseError(response)
      return false
    }
    return true
  }

  async function setItemState(id: string, subscriptionId: string, state: SubscriptionItemState): Promise<void> {
    if (await putItemState(id, state)) await reloadItems(subscriptionId)
    await loadReviewSummary()
  }

  /**
   * The same decision for the snapshot of every pending item in one subscription (RD-098-02).
   * The server owns that snapshot and performs queue hand-offs sequentially; failures remain
   * pending and are returned as a count while hits arriving later are untouched.
   */
  async function setPendingItemStates(
    subscriptionId: string,
    state: 'queued' | 'dismissed'
  ): Promise<SubscriptionBulkStateResponse | null> {
    const response = await api.PUT('/api/v1/subscriptions/{id}/items/pending', {
      params: { path: { id: subscriptionId } },
      body: { state }
    })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    await reloadItems(subscriptionId)
    await loadReviewSummary()
    return response.data
  }

  async function clearHistory(id: string): Promise<SubscriptionHistoryClearResponse | null> {
    const response = await api.DELETE('/api/v1/subscriptions/{id}/history', {
      params: { path: { id } }
    })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    await Promise.all([reloadItems(id), loadRuns(id), loadReviewSummary()])
    return response.data
  }

  return {
    subscriptions,
    items,
    itemPages,
    itemQueries,
    itemErrors,
    reviewSummary,
    runs,
    error,
    busy,
    loading,
    pollingIds,
    lastPoll,
    refresh,
    connectEvents,
    disconnectEvents,
    loadItems,
    loadMoreItems,
    loadReviewSummary,
    loadRuns,
    create,
    update,
    setEnabled,
    remove,
    loadCaps,
    probeCaps,
    pollNow,
    setItemState,
    setPendingItemStates,
    clearHistory
  }
})
