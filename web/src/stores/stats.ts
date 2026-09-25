import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'
import type { StatsRange, TransferStats } from '@/api/types'

/** A finished transfer changes the figures; a burst of them should cost one read, not one each. */
const EVENT_DEBOUNCE_MS = 2_000
/** The hour rolls over on its own; a timer keeps the current bucket honest without events. */
const POLL_MS = 60_000

/**
 * The persistent transfer statistics behind the statistics view (RD-110-01).
 *
 * One range at a time: the view offers four, and the server folds the buckets for whichever
 * is asked. The store refreshes on `download.state` events, debounced, and once a minute.
 */
export const useStatsStore = defineStore('stats', () => {
  const range = ref<StatsRange>('day')
  const stats = ref<TransferStats | null>(null)
  const error = ref<string | null>(null)
  const fetching = ref(false)
  /** True once the first fetch has settled, so "nothing here" is only said when it is true. */
  const settled = ref(false)
  const loading = computed(() => !settled.value)

  let releaseEvents: (() => void) | null = null
  let pollTimer: number | null = null
  let debounceTimer: number | null = null

  async function refresh(): Promise<void> {
    fetching.value = true
    const requested = range.value
    const response = await api.GET('/api/v1/stats/transfers', { params: { query: { range: requested } } })
    // A slower answer for a range the reader has already left must not overwrite the one
    // they are looking at.
    if (requested === range.value) {
      if (response.data) {
        stats.value = response.data
        error.value = null
      } else {
        error.value = responseError(response)
      }
    }
    fetching.value = false
    settled.value = true
  }

  async function setRange(next: StatsRange): Promise<void> {
    if (next === range.value) return
    range.value = next
    await refresh()
  }

  function scheduleRefresh(): void {
    if (debounceTimer !== null) return
    debounceTimer = window.setTimeout(() => {
      debounceTimer = null
      void refresh()
    }, EVENT_DEBOUNCE_MS)
  }

  function start(): void {
    void refresh()
    releaseEvents ??= subscribeEvents({ 'download.state': scheduleRefresh })
    pollTimer ??= window.setInterval(() => void refresh(), POLL_MS)
  }

  function stop(): void {
    releaseEvents?.()
    releaseEvents = null
    if (pollTimer !== null) window.clearInterval(pollTimer)
    pollTimer = null
    if (debounceTimer !== null) window.clearTimeout(debounceTimer)
    debounceTimer = null
  }

  return { range, stats, error, fetching, settled, loading, refresh, setRange, start, stop }
})
