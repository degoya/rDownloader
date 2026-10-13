import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

import { api } from '@/api/client'
import type { DownloadSummary } from '@/api/types'
import { usePostprocessStore } from '@/stores/postprocess'
import { useTransfersStore } from '@/stores/transfers'

/**
 * The queue's summary under the list and the post-processing queue above it, kept current while
 * the downloads view is open: read when it opens, again whenever a download or a package changes
 * state, and on a slow timer as a safety net for missed events.
 */
export function useDownloadsSummary() {
  const transfers = useTransfersStore()
  const postprocess = usePostprocessStore()
  const summary = ref<DownloadSummary | null>(null)
  let summaryTimer: ReturnType<typeof setInterval> | null = null
  let postprocessTimer: ReturnType<typeof setInterval> | null = null

  const stateFingerprint = computed(() => transfers.downloads.map(download => `${download.id}:${download.state}`).join('|'))
  const packageStateFingerprint = computed(() => transfers.packages.map(pkg => `${pkg.id}:${pkg.state ?? ''}`).join('|'))

  async function loadSummary(): Promise<void> {
    const response = await api.GET('/api/v1/downloads/summary')
    if (response.data) summary.value = response.data
  }

  onMounted(() => {
    void loadSummary()
    void postprocess.refresh()
    // The summary follows the SSE-driven store refresh (see the fingerprint watchers);
    // this is only a long safety net for storage figures and missed events.
    summaryTimer = setInterval(() => void loadSummary(), 120_000)
    // Safety net for the pipeline queue (missed events, pending → running transitions). Kept
    // deliberately slow: `postprocess.progress` events and the fingerprint watcher below already
    // drive this, and a 3s poll competed with real traffic for the browser's connection budget.
    postprocessTimer = setInterval(() => { if (postprocess.active) void postprocess.refresh() }, 15_000)
  })
  onUnmounted(() => {
    if (summaryTimer) clearInterval(summaryTimer)
    if (postprocessTimer) clearInterval(postprocessTimer)
  })
  // `download.state` / `package.state` events feed the store's debounced refresh (400 ms);
  // both fingerprints change with it, which keeps the summary reactive without a manual button.
  watch(stateFingerprint, () => void loadSummary())
  watch(packageStateFingerprint, () => {
    void loadSummary()
    void postprocess.refresh()
  })

  return summary
}
