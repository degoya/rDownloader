<script setup lang="ts">
/**
 * The interactive indexer search inside the LinkGrabber (RD-180-19).
 *
 * Present only while at least one indexer is enabled: without one there is nothing to search,
 * and a field that can only fail is a broken feature. One request per search and per page —
 * the button, never a keystroke, sends it, because every request counts against the indexer's
 * daily limit and some indexers cache an answer for ten minutes. A term the indexer would refuse
 * (one or two characters) is refused here before anything is sent.
 *
 * Chosen hits go to the server, which fetches each NZB with the indexer's key and imports it the
 * way an uploaded `.nzb` is imported, so they arrive in the list below for review like a file.
 * The key itself never reaches this component: a hit's address carries a placeholder for it.
 *
 * `f` puts the keyboard in the search field (`indexerSearchFocus.ts`); the panel hands that in
 * while its field is on the page and takes it back when the field goes.
 */
import { useToast } from '@nuxt/ui/composables'
import type { TableColumn } from '@nuxt/ui'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { IndexerSearchHit, IndexerSearchResponse } from '@/api/types'
import { setIndexerSearchFocusAction } from '@/composables/indexerSearchFocus'
import { translateServerMessage } from '@/i18n/server'
import { useIndexersStore } from '@/stores/indexers'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { formatBytes } from '@/utils/format'
import {
  DEFAULT_LIMIT, LIMITS, MAX_AGE_DAYS, MAX_QUERY_CHARS, MIN_QUERY_CHARS, ageInDays, hitKey,
  maxAgeDays, queryProblem, sortHits, type HitSortKey
} from '@/utils/indexerSearch'

/** `MAX_GRAB_ITEMS` in `crates/rd-api-intake/src/indexer_search.rs`. */
const MAX_GRAB = 50
const ALL = '__all__'

const { t } = useI18n()
const toast = useToast()
const indexers = useIndexersStore()
const nzb = useNzbImportsStore()

const query = ref('')
const indexerChoice = ref(ALL)
const categories = ref<string[]>([])
const maxAge = ref<string | number>('')
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

const available = computed(() => indexers.enabled.length > 0)

const indexerItems = computed(() => [
  { value: ALL, label: t('linkgrabber.search.all_indexers') },
  ...indexers.enabled.map(indexer => ({ value: indexer.id, label: indexer.name }))
])
const limitItems = LIMITS.map(value => ({ value, label: String(value) }))

const hits = computed(() => sortHits(result.value?.hits ?? [], sortKey.value, descending.value))
const failures = computed(() => (result.value?.indexers ?? []).filter(outcome => outcome.error))
const hasMore = computed(() => (result.value?.indexers ?? []).some(outcome => outcome.more))
const allSelected = computed(() => hits.value.length > 0 && hits.value.every(hit => selected.value.has(hitKey(hit))))
const someSelected = computed(() => selected.value.size > 0 && !allSelected.value)
const tooMany = computed(() => selected.value.size > MAX_GRAB)

const columns: TableColumn<IndexerSearchHit>[] = [
  { id: 'select' },
  { id: 'title' },
  { id: 'size' },
  { id: 'age' },
  { id: 'category' },
  { id: 'indexer' },
  { id: 'grabs' }
]
const sortable = computed<{ key: HitSortKey, label: string }[]>(() => [
  { key: 'title', label: t('linkgrabber.search.columns.title') },
  { key: 'size', label: t('linkgrabber.search.columns.size') },
  { key: 'age', label: t('linkgrabber.search.columns.age') },
  { key: 'category', label: t('linkgrabber.search.columns.category') },
  { key: 'indexer', label: t('linkgrabber.search.columns.indexer') },
  { key: 'grabs', label: t('linkgrabber.search.columns.grabs') }
])

function focusField(): void {
  field.value?.querySelector('input')?.focus()
}

// The key does something only while there is a field to put the keyboard in.
watch(available, on => setIndexerSearchFocusAction(on ? focusField : null), { immediate: true })
onUnmounted(() => setIndexerSearchFocusAction(null))
onMounted(() => void indexers.refresh())

function sortBy(key: HitSortKey): void {
  if (sortKey.value === key) descending.value = !descending.value
  else {
    sortKey.value = key
    // Big, many and new are what one looks for first; a name reads from A.
    descending.value = key === 'size' || key === 'grabs'
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
  const ageGiven = String(maxAge.value).trim() !== ''
  ageError.value = ageGiven && maxAgeDays(maxAge.value) === null
    ? translateServerMessage({ code: 'indexer.max_age_invalid', params: { maximum: String(MAX_AGE_DAYS) } })
    : null
  return !queryError.value && !ageError.value
}

async function search(start: number): Promise<void> {
  if (!validate()) return
  searching.value = true
  searchError.value = null
  const term = query.value.trim()
  const response = await api.POST('/api/v1/indexers/search', {
    body: {
      indexer_ids: indexerChoice.value === ALL ? [] : [indexerChoice.value],
      query: term || null,
      categories: categories.value.map(entry => entry.trim()).filter(Boolean),
      max_age_days: maxAgeDays(maxAge.value),
      hide_passworded: hidePassworded.value,
      limit: pageSize.value,
      offset: start
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
}

function close(): void {
  result.value = null
  searchError.value = null
  selected.value = new Set()
  offset.value = 0
}

async function grab(): Promise<void> {
  const items = hits.value
    .filter(hit => selected.value.has(hitKey(hit)))
    .map(hit => ({ indexer_id: hit.indexer_id, download: hit.download, title: hit.title }))
  if (!items.length || tooMany.value) return
  grabbing.value = true
  const response = await api.POST('/api/v1/indexers/grab', { body: { items } })
  grabbing.value = false
  if (!response.data) {
    toast.add({ title: responseError(response), color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  const { imports, failed } = response.data
  if (imports.length) {
    toast.add({ title: t('linkgrabber.search.grabbed', { count: imports.length }, imports.length), color: 'success', icon: 'i-lucide-file-check' })
  }
  for (const failure of failed) {
    toast.add({ title: t('linkgrabber.search.grab_failed', { title: failure.title }), description: translateServerMessage(failure.error), color: 'warning', icon: 'i-lucide-circle-alert' })
  }
  // What arrived leaves the selection; what failed stays ticked for another try.
  const failedTitles = new Set(failed.map(failure => failure.title))
  selected.value = new Set(hits.value.filter(hit => selected.value.has(hitKey(hit)) && failedTitles.has(hit.title)).map(hitKey))
  void nzb.refresh()
}

function ageLabel(hit: IndexerSearchHit): string {
  const days = ageInDays(hit.published_at)
  return days === null ? '—' : t('linkgrabber.search.age_days', { count: days }, days)
}
</script>

<template>
  <section v-if="available" class="border border-muted p-3" data-testid="indexer-search" :aria-label="t('linkgrabber.search.title')">
    <form class="flex flex-wrap items-start gap-2" @submit.prevent="search(0)">
      <div ref="field" class="min-w-60 flex-1">
        <UInput
          v-model="query"
          class="w-full"
          icon="i-lucide-search"
          :maxlength="MAX_QUERY_CHARS"
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
      </div>
      <USelect v-model="indexerChoice" :items="indexerItems" value-key="value" class="w-44" :aria-label="t('linkgrabber.search.indexer_label')" data-testid="indexer-search-indexer" />
      <UInputTags v-model="categories" class="w-48" :placeholder="t('linkgrabber.search.categories_placeholder')" :aria-label="t('linkgrabber.search.categories_label')" data-testid="indexer-search-categories" />
      <div class="w-32">
        <UInput
          v-model="maxAge"
          type="number"
          min="1"
          :max="MAX_AGE_DAYS"
          class="w-full"
          :placeholder="t('linkgrabber.search.max_age_label')"
          :aria-label="t('linkgrabber.search.max_age_label')"
          :color="ageError ? 'error' : undefined"
          data-testid="indexer-search-max-age"
        />
        <p v-if="ageError" class="mt-1 text-xs text-error">{{ ageError }}</p>
      </div>
      <USelect v-model="limit" :items="limitItems" value-key="value" class="w-24" :aria-label="t('linkgrabber.search.limit_label')" :title="t('linkgrabber.search.limit_label')" />
      <USwitch v-model="hidePassworded" class="self-center" size="sm" :label="t('linkgrabber.search.hide_passworded')" :ui="{ label: 'whitespace-nowrap' }" data-testid="indexer-search-hide-passworded" />
      <UButton type="submit" icon="i-lucide-search" :label="t('linkgrabber.search.submit')" :loading="searching" data-testid="indexer-search-submit" />
    </form>

    <UAlert v-if="searchError" class="mt-3" color="error" variant="subtle" :description="searchError" />
    <template v-if="result">
      <UAlert
        v-for="outcome in failures"
        :key="outcome.indexer_id"
        class="mt-3"
        color="warning"
        variant="subtle"
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
            @click="grab"
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
        sticky
        :data="hits"
        :columns="columns"
        :get-row-id="hitKey"
        data-testid="indexer-search-results"
      >
        <template #select-header><span class="sr-only">{{ t('common.actions.select_all') }}</span></template>
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
          <span class="flex min-w-0 items-center gap-2">
            <span class="truncate font-mono text-xs" :title="row.original.title">{{ row.original.title }}</span>
            <UBadge v-if="row.original.passworded" color="warning" variant="subtle" size="sm" icon="i-lucide-lock" :label="t('linkgrabber.search.passworded')" />
          </span>
        </template>
        <template #size-cell="{ row }"><span class="numeric text-xs">{{ row.original.size_bytes == null ? '—' : formatBytes(String(row.original.size_bytes)) }}</span></template>
        <template #age-cell="{ row }"><span class="numeric text-xs" :title="row.original.published_at ?? undefined">{{ ageLabel(row.original) }}</span></template>
        <template #category-cell="{ row }"><span class="font-mono text-xs">{{ row.original.category ?? '—' }}</span></template>
        <template #indexer-cell="{ row }"><span class="text-xs">{{ row.original.indexer_name }}</span></template>
        <template #grabs-cell="{ row }"><span class="numeric text-xs">{{ row.original.grabs ?? '—' }}</span></template>
      </UTable>
    </template>
  </section>
</template>
