import type { CommandPaletteGroup, CommandPaletteItem } from '@nuxt/ui'
import { watchDebounced } from '@vueuse/core'
import { ref, type Ref } from 'vue'

import { api } from '@/api/client'
import type { components } from '@/api/schema'
import { router } from '@/router'

type QueueSearchResponse = components['schemas']['QueueSearchResponse']
type Translate = (key: string) => string

/** Rows of each kind the palette asks for; the server caps them at 50. */
export const PALETTE_QUEUE_LIMIT = 8
/** Characters typed before the queue is asked: one letter matches half of it. */
export const PALETTE_QUEUE_MIN_TERM = 2
const DEBOUNCE_MS = 200

/** The download list, scrolled to one row and the keyboard on it (`useQueueReveal`). */
export function revealInQueue(key: string): void {
  void router.push({ path: '/downloads', query: { reveal: key } })
}

/**
 * The queue's packages and files under what the palette is asked (RD-1240-14), as two groups
 * after the views. The server matched them already, so the palette's own fuzzy filter is off
 * for these groups (`ignoreFilter`); a choice opens the download list on the row.
 */
export function queueSearchGroups(t: Translate, hits: QueueSearchResponse | null): CommandPaletteGroup<CommandPaletteItem>[] {
  if (!hits) return []
  const packages: CommandPaletteItem[] = hits.packages.map(pkg => ({
    id: `package:${pkg.id}`,
    label: pkg.name,
    icon: 'i-lucide-package',
    onSelect: () => revealInQueue(`package:${pkg.id}`)
  }))
  const downloads: CommandPaletteItem[] = hits.downloads.map(download => ({
    id: `file:${download.id}`,
    label: download.file_name,
    suffix: download.package_name,
    icon: 'i-lucide-file-down',
    onSelect: () => revealInQueue(`file:${download.id}`)
  }))
  return [
    ...(packages.length ? [{ id: 'queue-packages', label: t('nav.search.groups.packages'), ignoreFilter: true, items: packages }] : []),
    ...(downloads.length ? [{ id: 'queue-downloads', label: t('nav.search.groups.downloads'), ignoreFilter: true, items: downloads }] : [])
  ]
}

/**
 * Asks the server for the queue's rows under `term`, a moment after the last key; an answer to
 * an older term than the newest one asked is dropped, and a refusal or an outage shows no rows
 * rather than an error inside the palette.
 */
export function usePaletteQueueSearch(term: Ref<string>): { hits: Ref<QueueSearchResponse | null>, loading: Ref<boolean> } {
  const hits = ref<QueueSearchResponse | null>(null)
  const loading = ref(false)
  let asked = 0

  watchDebounced(term, async (value) => {
    const q = value.trim()
    const ticket = ++asked
    if (q.length < PALETTE_QUEUE_MIN_TERM) {
      hits.value = null
      loading.value = false
      return
    }
    loading.value = true
    const response = await api.GET('/api/v1/queue/search', { params: { query: { q, limit: PALETTE_QUEUE_LIMIT } } })
    if (ticket !== asked) return
    hits.value = response.data ?? null
    loading.value = false
  }, { debounce: DEBOUNCE_MS })

  return { hits, loading }
}
