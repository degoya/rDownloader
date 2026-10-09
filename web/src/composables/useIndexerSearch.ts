import { useToast } from '@nuxt/ui/composables'
import { computed, inject, onMounted, ref, type InjectionKey } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { IndexerSearchHit, IndexerSearchResponse } from '@/api/types'
import { useErrorToast } from '@/composables/useErrorToast'
import { useIndexerSearchType } from '@/composables/useIndexerSearchType'
import { translateServerMessage } from '@/i18n/server'
import { useCollectorStore } from '@/stores/collector'
import { useIndexersStore } from '@/stores/indexers'
import { useNzbImportsStore } from '@/stores/nzbImports'
import {
  DEFAULT_LIMIT, LIMITS, MAX_AGE_DAYS, MAX_QUERY_CHARS, MIN_QUERY_CHARS, ageInDays, hitCoverUrl,
  hitDescription, hitFacts, hitKey, maxAgeDays, queryProblem, sortHits, type HitSortKey
} from '@/utils/indexerSearch'
import { showItemImages } from '@/utils/itemImages'

/** `MAX_GRAB_ITEMS` in `crates/rd-api-intake/src/indexer_search.rs`. */
export const MAX_GRAB = 50
const ALL = '__all__'

/** Where one hit's own button stands; the busy state is the row's, never the table's. */
type RowState = 'pending' | 'done' | 'error'

/**
 * The state and the requests of the LinkGrabber's indexer search (RD-180-19), apart from its form.
 *
 * Since RD-1230-02 the form lives in a drawer whose content is unmounted while it is closed, so
 * the inputs, the hits and each row's state are held here, by the drawer that stays mounted with
 * the view: reopened, the search is as it was left. A panel rendered on its own (its tests) makes
 * its own through `useIndexerSearchState`.
 */
export function useIndexerSearch() {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()
  const indexers = useIndexersStore()
  const nzb = useNzbImportsStore()
  const collector = useCollectorStore()

  const query = ref('')
  const indexerChoice = ref(ALL)
  const categories = ref<string[]>([])
  const maxAge = ref<number | null | undefined>(null)
  const hidePassworded = ref(false)
  const limit = ref<number>(DEFAULT_LIMIT)
  /** The page size as a number, whatever the select handed back. */
  const pageSize = computed(() => Number(limit.value) || DEFAULT_LIMIT)

  const searching = ref(false)
  const grabbing = ref(false)
  const queryError = ref<string | null>(null)
  const ageError = ref<string | null>(null)
  const searchError = ref<string | null>(null)
  const result = ref<IndexerSearchResponse | null>(null)
  const offset = ref(0)
  const selected = ref<Set<string>>(new Set())
  const sortKey = ref<HitSortKey | null>(null)
  const descending = ref(false)
  const rowStates = ref<Map<string, RowState>>(new Map())
  /** Hits whose cover did not load; their row shows the placeholder instead of a broken picture. */
  const brokenCovers = ref<Set<string>>(new Set())

  const available = computed(() => indexers.enabled.length > 0)
  /** The indexers whose hits are drawn detailed; every other hit is compact, as before RD-190-16. */
  const detailedIndexers = computed(() =>
    new Set(indexers.indexers.filter(indexer => indexer.list_style === 'detailed').map(indexer => indexer.id)))
  /** The hint waits for the first answer, so it does not flash up while the list is loading. */
  const unavailable = computed(() => indexers.loaded && !available.value)

  const indexerItems = computed(() => [
    { value: ALL, label: t('linkgrabber.search.all_indexers') },
    ...indexers.enabled.map(indexer => ({ value: indexer.id, label: indexer.name }))
  ])
  const limitItems = LIMITS.map(value => ({ value, label: String(value) }))
  /** The indexers a free search goes to; a typed one asks those of them that answer it. */
  const targetIds = computed(() => indexerChoice.value === ALL ? indexers.enabled.map(indexer => indexer.id) : [indexerChoice.value])
  const { typed, typedError, capsList, askCaps, validateTyped, answeringIndexers, typedPart } = useIndexerSearchType(targetIds)

  const hits = computed(() => sortHits(result.value?.hits ?? [], sortKey.value, descending.value))
  const failures = computed(() => (result.value?.indexers ?? []).filter(outcome => outcome.error))
  const hasMore = computed(() => (result.value?.indexers ?? []).some(outcome => outcome.more))
  const allSelected = computed(() => hits.value.length > 0 && hits.value.every(hit => selected.value.has(hitKey(hit))))
  const someSelected = computed(() => selected.value.size > 0 && !allSelected.value)
  const tooMany = computed(() => selected.value.size > MAX_GRAB)

  onMounted(() => void indexers.refresh())

  function sortBy(key: HitSortKey): void {
    if (sortKey.value === key) descending.value = !descending.value
    else {
      sortKey.value = key
      // Big and new are what one looks for first; a name reads from A.
      descending.value = key === 'size'
    }
  }

  function sortIcon(key: HitSortKey): string {
    if (sortKey.value !== key) return 'i-lucide-arrow-up-down'
    return descending.value ? 'i-lucide-arrow-down-wide-narrow' : 'i-lucide-arrow-up-narrow-wide'
  }

  function toggle(hit: IndexerSearchHit, value: boolean | 'indeterminate'): void {
    const next = new Set(selected.value)
    if (value === true) next.add(hitKey(hit))
    else next.delete(hitKey(hit))
    selected.value = next
  }

  function toggleAll(): void {
    selected.value = allSelected.value ? new Set() : new Set(hits.value.map(hitKey))
  }

  /** Checks what can be checked here, so a search the indexer would refuse costs it nothing. */
  function validate(): boolean {
    const problem = queryProblem(query.value)
    queryError.value = problem
      ? translateServerMessage({ code: problem, params: { minimum: String(MIN_QUERY_CHARS), maximum: String(MAX_QUERY_CHARS) } })
      : null
    const ageGiven = maxAge.value != null
    ageError.value = ageGiven && maxAgeDays(maxAge.value) === null
      ? translateServerMessage({ code: 'indexer.max_age_invalid', params: { maximum: String(MAX_AGE_DAYS) } })
      : null
    return validateTyped() && !queryError.value && !ageError.value
  }

  async function search(start: number): Promise<void> {
    if (!validate()) return
    const indexerIds = typed.value.type === 'search' ? (indexerChoice.value === ALL ? [] : [indexerChoice.value]) : await answeringIndexers()
    searchError.value = indexerIds ? null : t('linkgrabber.search.type_unsupported')
    if (!indexerIds) return
    searching.value = true
    const term = query.value.trim()
    const response = await api.POST('/api/v1/indexers/search', {
      body: {
        indexer_ids: indexerIds,
        query: term || null,
        categories: categories.value.map(entry => entry.trim()).filter(Boolean),
        max_age_days: maxAgeDays(maxAge.value),
        hide_passworded: hidePassworded.value,
        limit: pageSize.value,
        offset: start,
        ...typedPart()
      }
    })
    searching.value = false
    if (!response.data) {
      searchError.value = responseError(response)
      return
    }
    result.value = response.data
    offset.value = start
    selected.value = new Set()
    brokenCovers.value = new Set()
  }

  function close(): void {
    result.value = null
    searchError.value = null
    selected.value = new Set()
    offset.value = 0
    rowStates.value = new Map()
    brokenCovers.value = new Set()
  }

  function markRows(chosen: readonly IndexerSearchHit[], state: (hit: IndexerSearchHit) => RowState): void {
    const next = new Map(rowStates.value)
    for (const hit of chosen) next.set(hitKey(hit), state(hit))
    rowStates.value = next
  }

  /** The ticked hits, through the same route one row's button takes. */
  async function grabSelected(): Promise<void> {
    if (tooMany.value) return
    grabbing.value = true
    await grab(hits.value.filter(hit => selected.value.has(hitKey(hit))))
    grabbing.value = false
  }

  async function grab(chosen: readonly IndexerSearchHit[]): Promise<void> {
    if (!chosen.length) return
    const items = chosen.map(hit => ({ indexer_id: hit.indexer_id, download: hit.download, title: hit.title, ...(hit.magnet ? { magnet: hit.magnet } : {}) }))
    markRows(chosen, () => 'pending')
    const response = await api.POST('/api/v1/indexers/grab', { body: { items } })
    if (!response.data) {
      markRows(chosen, () => 'error')
      showError(responseError(response))
      return
    }
    const { imports, failed } = response.data
    const torrents = response.data.torrents ?? []
    if (imports.length) toast.add({ title: t('linkgrabber.search.grabbed', { count: imports.length }, imports.length), color: 'success', icon: 'i-lucide-file-check' })
    if (torrents.length) toast.add({ title: t('linkgrabber.search.grabbed_torrents', { count: torrents.length }, torrents.length), color: 'success', icon: 'i-lucide-magnet' })
    if (torrents.length) void collector.refresh()
    for (const failure of failed) {
      toast.add({ title: t('linkgrabber.search.grab_failed', { title: failure.title }), description: translateServerMessage(failure.error), color: 'warning', icon: 'i-lucide-circle-alert' })
    }
    // What arrived leaves the selection; what failed stays ticked for another try.
    const failedTitles = new Set(failed.map(failure => failure.title))
    markRows(chosen, hit => failedTitles.has(hit.title) ? 'error' : 'done')
    const arrived = new Set(chosen.filter(hit => !failedTitles.has(hit.title)).map(hitKey))
    selected.value = new Set([...selected.value].filter(key => !arrived.has(key)))
    void nzb.refresh()
  }

  function rowState(hit: IndexerSearchHit): RowState | undefined {
    return rowStates.value.get(hitKey(hit))
  }

  function rowIcon(hit: IndexerSearchHit): string {
    const state = rowState(hit)
    if (state === 'done') return 'i-lucide-check'
    if (state === 'error') return 'i-lucide-circle-alert'
    return 'i-lucide-download'
  }

  function rowColor(hit: IndexerSearchHit): 'success' | 'error' | 'primary' {
    const state = rowState(hit)
    return state === 'done' ? 'success' : state === 'error' ? 'error' : 'primary'
  }

  function rowLabel(hit: IndexerSearchHit): string {
    const state = rowState(hit)
    const key = state === 'done' ? 'grab_one_done' : state === 'error' ? 'grab_one_failed' : 'grab_one'
    return t(`linkgrabber.search.${key}`, { title: hit.title })
  }

  function detailed(hit: IndexerSearchHit): boolean {
    return detailedIndexers.value.has(hit.indexer_id)
  }

  /** The cover a detailed row loads: only with pictures allowed, and never one that failed. */
  function coverOf(hit: IndexerSearchHit): string | null {
    return brokenCovers.value.has(hitKey(hit)) ? null : hitCoverUrl(hit, showItemImages.value)
  }

  function dropCover(hit: IndexerSearchHit): void {
    brokenCovers.value = new Set(brokenCovers.value).add(hitKey(hit))
  }

  /** The whole metadata line as one sentence, for the tooltip of the line that is drawn cut. */
  function metadataTitle(hit: IndexerSearchHit): string {
    const facts = hitFacts(hit, t).map(fact => `${fact.label} ${fact.value}`)
    const description = hitDescription(hit)
    return [...facts, ...(description ? [description] : [])].join(' · ')
  }

  function ageLabel(hit: IndexerSearchHit): string {
    const days = ageInDays(hit.published_at)
    return days === null ? '—' : t('linkgrabber.search.age_days', { count: days }, days)
  }

  return {
    query, indexerChoice, categories, maxAge, hidePassworded, limit, pageSize, searching, grabbing,
    queryError, ageError, searchError, result, offset, selected, available, unavailable, indexerItems,
    limitItems, typed, typedError, capsList, askCaps, hits, failures, hasMore, allSelected, someSelected,
    tooMany, sortKey, sortBy, sortIcon, toggle, toggleAll, search, close, grab, grabSelected, rowState,
    rowIcon, rowColor, rowLabel, detailed, coverOf, dropCover, metadataTitle, ageLabel
  }
}

export type IndexerSearchState = ReturnType<typeof useIndexerSearch>

export const INDEXER_SEARCH_STATE: InjectionKey<IndexerSearchState> = Symbol('indexer-search')

/** The drawer's state when the panel is inside it; a panel on its own makes its own. */
export function useIndexerSearchState(): IndexerSearchState {
  return inject(INDEXER_SEARCH_STATE, () => useIndexerSearch(), true)
}
