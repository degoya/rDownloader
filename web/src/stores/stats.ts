import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useLatestFetch } from '@/composables/useLatestFetch'
import type { StatsRange, TransferStats, UsenetServerTraffic } from '@/api/types'

/** A finished transfer changes the figures; a burst of them should cost one read, not one each. */
const EVENT_DEBOUNCE_MS = 2_000
/** The hour rolls over on its own; a timer keeps the current bucket honest without events. */
const POLL_MS = 60_000

/**
 * The persistent transfer statistics behind the statistics view (RD-110-01).
 *
 * One range at a time: the view offers four, and the server folds the buckets for whichever
 * is asked. The store refreshes on `download.state` events, debounced, and once a minute.
 *
 * The traffic per Usenet server (RD-1100-05) is read beside it: fixed ranges, independent of the
 * chosen one, and refreshed on `usenet.changed` too, which is what a used-up quota announces.
 */
export const useStatsStore = defineStore('stats', () => {
  const range = ref<StatsRange>('day')
  const stats = ref<TransferStats | null>(null)
  /** Every configured Usenet server with what it delivered; empty without servers. */
  const servers = ref<UsenetServerTraffic[]>([])
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  /** `settled` says "nothing here" only once it is true; only the newest read lands (WEB-06). */
  const { fetching, settled, loading, run } = useLatestFetch()

  let pollTimer: number | null = null

  async function refreshServers(): Promise<void> {
    // A failure keeps the last figures; the range's own read reports the outage.
    const response = await api.GET('/api/v1/stats/usenet-servers')
    if (response.data) servers.value = response.data.servers
  }

  async function refresh(): Promise<void> {
    void refreshServers()
    // The ticket in `run` keeps a slower answer — for a range the reader has already left, or an
    // older read of the same one from the poll — from overwriting the one they are looking at.
    await run(() => api.GET('/api/v1/stats/transfers', { params: { query: { range: range.value } } }), (response) => {
      if (response.data) {
        stats.value = response.data
        error.value = null
      } else {
        error.value = responseError(response)
      }
    })
  }

  async function setRange(next: StatsRange): Promise<void> {
    if (next === range.value) return
    range.value = next
    await refresh()
  }

  const events = debouncedEventRefresh(['download.state', 'usenet.changed'], refresh, { delayMs: EVENT_DEBOUNCE_MS })

  function start(): void {
    void refresh()
    events.connect()
    pollTimer ??= window.setInterval(() => void refresh(), POLL_MS)
  }

  function stop(): void {
    events.disconnect()
    if (pollTimer !== null) window.clearInterval(pollTimer)
    pollTimer = null
  }

  return { range, stats, servers, error, fetching, settled, loading, refresh, setRange, start, stop }
})
