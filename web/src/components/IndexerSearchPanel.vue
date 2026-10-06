<script setup lang="ts">
/**
 * The interactive indexer search inside the LinkGrabber (RD-180-19).
 *
 * Always present, so the search is found where it is used (owner, 2026-10-01). Without an enabled
 * indexer there is nothing to search: the field is disabled and a hint under it leads to
 * Settings › Usenet › Indexers — an indexer subscription alone is not searched. One request per
 * search and per page —
 * the button, never a keystroke, sends it, because every request counts against the indexer's
 * daily limit and some indexers cache an answer for ten minutes. A term the indexer would refuse
 * (one or two characters) is refused here before anything is sent.
 *
 * Chosen hits — the ticked ones, or one row's own button — go to the server, which fetches each
 * NZB with the indexer's key and imports it the way an uploaded `.nzb` is imported, so they
 * arrive in the list below for review like a file; a Torznab torrent (RD-1100-03) arrives as a
 * package, like a pasted magnet. The key never reaches this component: a hit's address carries a
 * placeholder for it. A TV or film search goes only to the indexers that answer it.
 *
 * `f` puts the keyboard in the search field (`indexerSearchFocus.ts`), or, while the field is
 * disabled, on the hint's link — a disabled field cannot take the focus, and the link is the one
 * thing to do there. The panel hands that in while it is mounted.
 *
 * Each indexer draws its hits in the list style it was given in the settings (RD-190-16): compact,
 * one line, or detailed, with a small cover and a line of the metadata the indexer sent. Under
 * "all indexers" every hit takes its own indexer's style. A detailed row without a cover — none
 * sent, pictures switched off, or one that did not load — carries the subscription lists'
 * placeholder, so the titles of all detailed rows start in one place.
 */
import { useToast } from '@nuxt/ui/composables'
import type { TableColumn } from '@nuxt/ui'
import { computed, onMounted, onUnmounted, ref, useId } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { IndexerSearchHit, IndexerSearchResponse } from '@/api/types'
import CoverPlaceholder from '@/components/CoverPlaceholder.vue'
import IndexerHitBadges from '@/components/IndexerHitBadges.vue'
import IndexerSearchTypeFields from '@/components/IndexerSearchTypeFields.vue'
import { setIndexerSearchFocusAction } from '@/composables/indexerSearchFocus'
import { useErrorToast } from '@/composables/useErrorToast'
import { useIndexerSearchType } from '@/composables/useIndexerSearchType'
import { translateServerMessage } from '@/i18n/server'
import { useCollectorStore } from '@/stores/collector'
import { useIndexersStore } from '@/stores/indexers'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { formatBytes } from '@/utils/format'
import {
  DEFAULT_LIMIT, LIMITS, MAX_AGE_DAYS, MAX_QUERY_CHARS, MIN_QUERY_CHARS, ageInDays, hitCoverUrl,
  hitDescription, hitFacts, hitKey, maxAgeDays, queryProblem, sortHits, type HitSortKey
} from '@/utils/indexerSearch'
import { showItemImages } from '@/utils/itemImages'
import { WHOLE } from '@/utils/numberInput'

/** `MAX_GRAB_ITEMS` in `crates/rd-api-intake/src/indexer_search.rs`. */
const MAX_GRAB = 50
const ALL = '__all__'

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
const field = ref<HTMLElement | null>(null)
const hint = ref<HTMLElement | null>(null)
const hintId = useId()

/** Where one hit's own button stands; the busy state is the row's, never the table's. */
type RowState = 'pending' | 'done' | 'error'
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

/**
 * A fixed layout (1.8.1): every column but the title has a narrow width of its own and the title
 * takes the rest, cut with an ellipsis, so a long release name can no longer push the row's own
 * button out of the panel. Below `sm` the table keeps a minimum width and scrolls sideways, and
 * the button column stays pinned to the right edge so it is always in reach.
 */
const GRAB_CELL = 'w-14 max-sm:sticky max-sm:right-0 max-sm:bg-default'
const columns: TableColumn<IndexerSearchHit>[] = [
  { id: 'select', meta: { class: { th: 'w-12', td: 'w-12' } } },
  { id: 'title' },
  { id: 'size', meta: { class: { th: 'w-28', td: 'w-28' } } },
  { id: 'age', meta: { class: { th: 'w-32', td: 'w-32' } } },
  { id: 'category', meta: { class: { th: 'w-36', td: 'w-36' } } },
  { id: 'grab', meta: { class: { th: GRAB_CELL, td: GRAB_CELL } } }
]
const sortable = computed<{ key: HitSortKey, label: string }[]>(() => [
  { key: 'title', label: t('linkgrabber.search.columns.title') },
  { key: 'size', label: t('linkgrabber.search.columns.size') },
  { key: 'age', label: t('linkgrabber.search.columns.age') },
  { key: 'category', label: t('linkgrabber.search.columns.category') }
])

function focusField(): void {
  if (available.value) field.value?.querySelector('input')?.focus()
  else hint.value?.querySelector('a')?.focus()
}

setIndexerSearchFocusAction(focusField)
onUnmounted(() => setIndexerSearchFocusAction(null))
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
</script>

<template>
  <section class="border border-muted p-3" data-testid="indexer-search" :aria-label="t('linkgrabber.search.title')">
    <form class="flex flex-wrap items-start gap-2" @submit.prevent="search(0)">
      <div ref="field" class="min-w-60 flex-1">
        <UInput
          v-model="query"
          class="w-full"
          icon="i-lucide-search"
          :maxlength="MAX_QUERY_CHARS"
          :disabled="!available"
          :aria-describedby="unavailable ? hintId : undefined"
          :placeholder="t('linkgrabber.search.query_placeholder')"
          :aria-label="t('linkgrabber.search.query_label')"
          :aria-invalid="queryError ? true : undefined"
          :color="queryError ? 'error' : undefined"
          autocomplete="off"
          data-testid="indexer-search-query"
        >
          <template #trailing><UKbd value="f" /></template>
        </UInput>
        <p v-if="queryError" class="mt-1 text-xs text-error" data-testid="indexer-search-query-error">{{ queryError }}</p>
        <p v-if="unavailable" :id="hintId" ref="hint" class="mt-1 text-xs text-muted" data-testid="indexer-search-unavailable">
          {{ t('linkgrabber.search.unavailable') }}
          <ULink to="/settings/usenet?tab=indexers" class="text-primary underline">{{ t('linkgrabber.search.unavailable_link') }}</ULink>
        </p>
      </div>
      <USelect v-model="indexerChoice" :items="indexerItems" value-key="value" class="w-44" :disabled="!available" :aria-label="t('linkgrabber.search.indexer_label')" data-testid="indexer-search-indexer" />
      <IndexerSearchTypeFields v-model="typed" :caps-list="capsList" :disabled="!available" :error="typedError" @ask="askCaps" />
      <UInputTags v-model="categories" class="w-48" :disabled="!available" :placeholder="t('linkgrabber.search.categories_placeholder')" :aria-label="t('linkgrabber.search.categories_label')" data-testid="indexer-search-categories" />
      <div class="w-48">
        <UInputNumber
          v-model="maxAge"
          :min="1"
          :max="MAX_AGE_DAYS"
          :format-options="WHOLE"
          :disabled="!available"
          class="w-full"
          :placeholder="t('linkgrabber.search.max_age_label')"
          :aria-label="t('linkgrabber.search.max_age_label')"
          :color="ageError ? 'error' : undefined"
          data-testid="indexer-search-max-age"
        />
        <p v-if="ageError" class="mt-1 text-xs text-error">{{ ageError }}</p>
      </div>
      <USelect v-model="limit" :items="limitItems" value-key="value" class="w-24" :disabled="!available" :aria-label="t('linkgrabber.search.limit_label')" :title="t('linkgrabber.search.limit_label')" />
      <USwitch v-model="hidePassworded" class="self-center" size="sm" :disabled="!available" :label="t('linkgrabber.search.hide_passworded')" :ui="{ label: 'whitespace-nowrap' }" data-testid="indexer-search-hide-passworded" />
      <UButton type="submit" icon="i-lucide-search" :label="t('linkgrabber.search.submit')" :disabled="!available" :loading="searching" data-testid="indexer-search-submit" />
    </form>

    <UAlert v-if="searchError" class="mt-3" color="error" :description="searchError" />
    <template v-if="result">
      <UAlert
        v-for="outcome in failures"
        :key="outcome.indexer_id"
        class="mt-3"
        color="warning"
        icon="i-lucide-circle-alert"
        :title="t('linkgrabber.search.indexer_failed', { name: outcome.indexer_name })"
        :description="translateServerMessage(outcome.error)"
        data-testid="indexer-search-outcome-error"
      />
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <UCheckbox
          :model-value="allSelected ? true : someSelected ? 'indeterminate' : false"
          :disabled="!hits.length"
          :label="t('common.actions.select_all')"
          :ui="{ label: 'whitespace-nowrap' }"
          @update:model-value="toggleAll"
        />
        <span class="numeric text-xs text-muted" role="status">{{ t('linkgrabber.search.results', { count: hits.length }, hits.length) }} · {{ t('linkgrabber.search.page', { page: Math.floor(offset / pageSize) + 1 }) }}</span>
        <div class="ml-auto flex flex-wrap items-center gap-2">
          <UButton
            size="sm"
            icon="i-lucide-list-plus"
            :label="t('linkgrabber.search.grab', { count: selected.size })"
            :title="tooMany ? translateServerMessage({ code: 'indexer.grab_too_many', params: { maximum: String(MAX_GRAB) } }) : t('linkgrabber.search.grab_hint')"
            :disabled="!selected.size || tooMany"
            :loading="grabbing"
            data-testid="indexer-search-grab"
            @click="grabSelected"
          />
          <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-chevron-left" :aria-label="t('linkgrabber.search.previous')" :title="t('linkgrabber.search.previous')" :disabled="offset === 0 || searching" data-testid="indexer-search-previous" @click="search(Math.max(0, offset - pageSize))" />
          <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-chevron-right" :aria-label="t('linkgrabber.search.next')" :title="t('linkgrabber.search.next')" :disabled="!hasMore || searching" data-testid="indexer-search-next" @click="search(offset + pageSize)" />
          <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('linkgrabber.search.close')" :title="t('linkgrabber.search.close')" @click="close" />
        </div>
      </div>
      <p v-if="!hits.length && !failures.length" class="mt-3 text-sm text-muted">{{ t('linkgrabber.search.empty') }}</p>
      <UTable
        v-else-if="hits.length"
        class="mt-2 max-h-[60vh]"
        :ui="{ base: 'w-full table-fixed max-sm:min-w-[40rem]' }"
        sticky
        :data="hits"
        :columns="columns"
        :get-row-id="hitKey"
        data-testid="indexer-search-results"
      >
        <template #select-header><span class="sr-only">{{ t('common.actions.select_all') }}</span></template>
        <template #grab-header><span class="sr-only">{{ t('linkgrabber.search.grab_hint') }}</span></template>
        <template v-for="column in sortable" :key="column.key" #[`${column.key}-header`]>
          <UButton
            size="xs"
            color="neutral"
            variant="ghost"
            class="-mx-2"
            :label="column.label"
            :trailing-icon="sortIcon(column.key)"
            :aria-label="t('linkgrabber.search.sort_by', { column: column.label })"
            :aria-pressed="sortKey === column.key"
            :data-testid="`indexer-search-sort-${column.key}`"
            @click="sortBy(column.key)"
          />
        </template>
        <template #select-cell="{ row }">
          <UCheckbox
            :model-value="selected.has(hitKey(row.original))"
            :aria-label="t('linkgrabber.search.select_hit', { title: row.original.title })"
            @update:model-value="(value: boolean | 'indeterminate') => toggle(row.original, value)"
          />
        </template>
        <template #title-cell="{ row }">
          <!-- Detailed (RD-190-16): the cover at the thumbnail size every list uses, then the title
               over one line of metadata. The cell stays in the title column, so size, age,
               category and the button keep their place in either style. -->
          <div v-if="detailed(row.original)" class="flex min-w-0 items-center gap-2" data-testid="indexer-search-hit-detailed">
            <img
              v-if="coverOf(row.original)"
              :src="coverOf(row.original) ?? undefined"
              alt=""
              loading="lazy"
              decoding="async"
              class="size-12 shrink-0 bg-elevated object-cover"
              data-testid="indexer-search-hit-cover"
              @error="dropCover(row.original)"
            >
            <CoverPlaceholder v-else class="size-12" />
            <div class="min-w-0 flex-1">
              <span class="flex min-w-0 items-center gap-2">
                <span class="min-w-0 truncate font-mono text-xs" :title="row.original.title" data-testid="indexer-search-hit-title">{{ row.original.title }}</span>
                <IndexerHitBadges :hit="row.original" />
              </span>
              <!-- One line, cut at the cell's edge; the tooltip holds all of it. -->
              <div
                v-if="metadataTitle(row.original)"
                class="mt-0.5 flex min-w-0 items-baseline gap-x-3 overflow-hidden whitespace-nowrap text-2xs"
                :title="metadataTitle(row.original)"
                data-testid="indexer-search-hit-metadata"
              >
                <dl v-if="hitFacts(row.original, t).length" class="flex shrink-0 items-baseline gap-x-3">
                  <div v-for="fact in hitFacts(row.original, t)" :key="fact.key" class="flex items-baseline gap-1">
                    <dt class="text-muted">{{ fact.label }}</dt>
                    <dd class="numeric text-highlighted">{{ fact.value }}</dd>
                  </div>
                </dl>
                <span v-if="hitDescription(row.original)" class="min-w-0 truncate text-muted">{{ hitDescription(row.original) }}</span>
              </div>
            </div>
          </div>
          <!-- The whole name stays in the DOM, only drawn cut: a screen reader reads all of it, and
               the tooltip shows it. The badge never shrinks, so the ellipsis cannot take it. -->
          <span v-else class="flex min-w-0 items-center gap-2">
            <span class="min-w-0 truncate font-mono text-xs" :title="row.original.title" data-testid="indexer-search-hit-title">{{ row.original.title }}</span>
            <IndexerHitBadges :hit="row.original" />
          </span>
        </template>
        <template #size-cell="{ row }"><span class="numeric text-xs">{{ row.original.size_bytes == null ? '—' : formatBytes(String(row.original.size_bytes)) }}</span></template>
        <template #age-cell="{ row }"><span class="numeric text-xs" :title="row.original.published_at ?? undefined">{{ ageLabel(row.original) }}</span></template>
        <template #category-cell="{ row }"><span class="block truncate font-mono text-xs" :title="row.original.category ?? undefined">{{ row.original.category ?? '—' }}</span></template>
        <template #grab-cell="{ row }">
          <UButton
            size="xs"
            variant="ghost"
            :color="rowColor(row.original)"
            :icon="rowIcon(row.original)"
            :aria-label="rowLabel(row.original)"
            :title="rowLabel(row.original)"
            :loading="rowState(row.original) === 'pending'"
            :disabled="rowState(row.original) === 'pending' || rowState(row.original) === 'done'"
            data-testid="indexer-search-grab-one"
            @click="grab([row.original])"
          />
        </template>
      </UTable>
    </template>
  </section>
</template>
