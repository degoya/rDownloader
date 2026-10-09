<script setup lang="ts">
/**
 * The interactive indexer search inside the LinkGrabber (RD-180-19).
 *
 * Always reachable, so the search is found where it is used (owner, 2026-10-01; since RD-1230-02
 * behind the navbar button and `f`, owner 2026-10-09). Without an enabled indexer there is nothing
 * to search: the field is disabled and a hint under it leads to
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
 * Since RD-1230-02 it is the body of `IndexerSearchDrawer`, which `f` and the navbar button open;
 * the drawer then calls `focusField`, which puts the keyboard in the search field or, while the
 * field is disabled, on the hint's link — a disabled field cannot take the focus, and the link is
 * the one thing to do there. Its state lives in `useIndexerSearch`, so a closed drawer keeps it.
 *
 * Each indexer draws its hits in the list style it was given in the settings (RD-190-16): compact,
 * one line, or detailed, with a small cover and a line of the metadata the indexer sent. Under
 * "all indexers" every hit takes its own indexer's style. A detailed row without a cover — none
 * sent, pictures switched off, or one that did not load — carries the subscription lists'
 * placeholder, so the titles of all detailed rows start in one place.
 */
import type { TableColumn } from '@nuxt/ui'
import { computed, ref, useId } from 'vue'
import { useI18n } from 'vue-i18n'

import type { IndexerSearchHit } from '@/api/types'
import CoverPlaceholder from '@/components/CoverPlaceholder.vue'
import IndexerHitBadges from '@/components/IndexerHitBadges.vue'
import IndexerSearchTypeFields from '@/components/IndexerSearchTypeFields.vue'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { MAX_GRAB, useIndexerSearchState } from '@/composables/useIndexerSearch'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes } from '@/utils/format'
import { MAX_AGE_DAYS, MAX_QUERY_CHARS, hitDescription, hitFacts, hitKey, type HitSortKey } from '@/utils/indexerSearch'
import { WHOLE } from '@/utils/numberInput'

const { t } = useI18n()
const {
  query, indexerChoice, categories, maxAge, hidePassworded, limit, pageSize, searching, grabbing,
  queryError, ageError, searchError, result, offset, selected, available, unavailable, indexerItems,
  limitItems, typed, typedError, capsList, askCaps, hits, failures, hasMore, allSelected, someSelected,
  tooMany, sortKey, sortBy, sortIcon, toggle, toggleAll, search, close, grab, grabSelected, rowState,
  rowIcon, rowColor, rowLabel, detailed, coverOf, dropCover, metadataTitle, ageLabel
} = useIndexerSearchState()

const field = ref<HTMLElement | null>(null)
const hint = ref<HTMLElement | null>(null)
const hintId = useId()

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

/** The field, or without an indexer the hint's link: the one thing there is to do then. */
function focusField(): void {
  if (available.value) field.value?.querySelector('input')?.focus()
  else hint.value?.querySelector('a')?.focus()
}

defineExpose({ focusField })
</script>

<template>
  <section data-testid="indexer-search" :aria-label="t('linkgrabber.search.title')">
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
      <SearchableSelect v-model="indexerChoice" :items="indexerItems" class="w-44" :disabled="!available" :aria-label="t('linkgrabber.search.indexer_label')" data-testid="indexer-search-indexer" />
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
