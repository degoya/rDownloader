import { watchDebounced } from '@vueuse/core'
import { computed, ref, watch, type ComputedRef, type Ref } from 'vue'
import { useRoute, useRouter, type LocationQuery } from 'vue-router'

import type { Download, DownloadPackage, DownloadState } from '@/api/types'

/** The download list's state filters, in the order the select offers them (RD-190-21). */
export const QUEUE_FILTERS = ['all', 'active', 'queued', 'paused', 'failed', 'seeding', 'completed'] as const
export type QueueFilter = typeof QUEUE_FILTERS[number]

/**
 * The states behind each filter. `queued` takes the files waiting for their next attempt too,
 * and `failed` everything that stopped short and wants a look: a blocked file and a cancelled
 * one are as stuck as a failed one, which is why the package header counts them as errors.
 */
const FILTER_STATES: Record<Exclude<QueueFilter, 'all'>, readonly DownloadState[]> = {
  active: ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'],
  queued: ['queued', 'retry_wait'],
  paused: ['paused'],
  failed: ['failed', 'blocked', 'cancelled'],
  seeding: ['seeding'],
  completed: ['completed']
}

/** How long the name search waits after the last key before it narrows the list. */
export const SEARCH_DEBOUNCE_MS = 200

function isQueueFilter(value: unknown): value is QueueFilter {
  return typeof value === 'string' && (QUEUE_FILTERS as readonly string[]).includes(value)
}

function queryText(value: LocationQuery[string] | undefined): string {
  const first = Array.isArray(value) ? value[0] : value
  return typeof first === 'string' ? first.trim() : ''
}

/**
 * The files the list shows for a filter and a search.
 *
 * The search matches the file name or the package name, case-insensitively: a hit on the package
 * keeps every file of it, because the package is what somebody remembers the name of. Without a
 * filter and a search the store's own array comes back, so nothing downstream recomputes for a
 * list that did not change.
 */
export function filterQueue(
  downloads: Download[],
  packages: readonly DownloadPackage[],
  filter: QueueFilter,
  search: string
): Download[] {
  const states = filter === 'all' ? null : FILTER_STATES[filter]
  const needle = search.trim().toLocaleLowerCase()
  if (!states && !needle) return downloads
  const packageHits = needle
    ? new Set(packages.filter(pkg => pkg.name.toLocaleLowerCase().includes(needle)).map(pkg => pkg.id))
    : null
  return downloads.filter(download =>
    (!states || states.includes(download.state))
    && (!packageHits || packageHits.has(download.package_id) || download.file_name.toLocaleLowerCase().includes(needle)))
}

/**
 * The download list's filter and name search, kept in the address as `?filter=` and `?q=`
 * (RD-190-21).
 *
 * A link to `/downloads?filter=failed` opens the list on its failed files, and a reload keeps
 * what was being looked at. The default carries no query, so the plain address stays plain, and
 * the address is replaced rather than pushed: narrowing a list is not a place back should step
 * through key by key. `search` is what the field holds; `needle` follows it after a pause, and
 * only `needle` filters, so a few thousand rows are not walked again on every key.
 */
export function useQueueFilter(): {
  filter: Ref<QueueFilter>
  search: Ref<string>
  needle: Ref<string>
  active: ComputedRef<boolean>
  reset: () => void
} {
  const route = useRoute()
  const router = useRouter()
  const initialFilter = route.query.filter
  const filter = ref<QueueFilter>(isQueueFilter(initialFilter) ? initialFilter : 'all')
  const search = ref(queryText(route.query.q))
  const needle = ref(search.value)
  const active = computed(() => filter.value !== 'all' || needle.value !== '')

  watchDebounced(search, (value) => { needle.value = value.trim() }, { debounce: SEARCH_DEBOUNCE_MS })

  watch([filter, needle], ([nextFilter, nextNeedle]) => {
    const query: LocationQuery = { ...route.query }
    delete query.filter
    delete query.q
    if (nextFilter !== 'all') query.filter = nextFilter
    if (nextNeedle) query.q = nextNeedle
    if (route.query.filter === query.filter && queryText(route.query.q) === (query.q ?? '')) return
    void router.replace({ path: route.path, query, hash: route.hash })
  })

  // A link followed while the list is open (`?filter=failed` from elsewhere) lands here too. The
  // search is compared with `needle`, not with the field: a replace that is still on its way
  // carries an older text than the one being typed, and must not type over it.
  watch(() => [route.query.filter, route.query.q] as const, ([nextFilter, nextQuery]) => {
    const wanted = isQueueFilter(nextFilter) ? nextFilter : 'all'
    if (wanted !== filter.value) filter.value = wanted
    const text = queryText(nextQuery)
    if (text !== needle.value) {
      search.value = text
      needle.value = text
    }
  })

  function reset(): void {
    filter.value = 'all'
    search.value = ''
    needle.value = ''
  }

  return { filter, search, needle, active, reset }
}
