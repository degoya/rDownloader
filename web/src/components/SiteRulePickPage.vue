<script setup lang="ts">
/**
 * One series page whose releases wait for a choice (RD-1170-03).
 *
 * The releases are grouped by season, with a checkbox per season that takes every shown release
 * of it, and four quick filters — season, episode, resolution, language — over the page's own
 * values. "Fetch links" resolves only what is ticked, one release after the other with one
 * captcha each; the header then counts "3 of 8" and says when a captcha waits for a person. A
 * release that is done or underway cannot be ticked again; one whose captcha went unanswered can.
 * "Take all" fetches every release that can still be fetched, filters or not — for a page whose
 * releases need no captcha, such as warez.cx's, the choice is a convenience (RD-1190-17).
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CollectorPick, CollectorPickEntry } from '@/api/types'
import {
  ALL, FILTERED, NO_EPISODE, attributeValues, noFilters, passes, seasonGroups, selectable,
  type FilteredAttribute, type PickFilters, type SeasonGroup
} from '@/utils/sitePicks'

const props = defineProps<{ page: CollectorPick, busy: boolean }>()
const emit = defineEmits<{
  resolve: [entries: number[]]
  cancel: []
  discard: []
}>()

const { t } = useI18n()

const filters = ref<PickFilters>(noFilters())
const picked = ref<Set<number>>(new Set())

const title = computed(() => props.page.package_name ?? props.page.address)
const shown = computed(() => props.page.entries.filter(entry => passes(entry, filters.value)))
const groups = computed(() => seasonGroups(shown.value))
/** Only what can still be resolved counts as ticked, whatever arrived since it was ticked. */
const chosen = computed(() => props.page.entries.filter(entry => picked.value.has(entry.index) && selectable(entry)))
/** Every release "Take all" would fetch. */
const open = computed(() => props.page.entries.filter(selectable))

const FILTER_KEYS: Record<FilteredAttribute, { label: string, all: string }> = {
  season: { label: 'linkgrabber.picks.filter.season', all: 'linkgrabber.picks.filter.all_seasons' },
  episode: { label: 'linkgrabber.picks.filter.episode', all: 'linkgrabber.picks.filter.all_episodes' },
  resolution: { label: 'linkgrabber.picks.filter.resolution', all: 'linkgrabber.picks.filter.all_resolutions' },
  language: { label: 'linkgrabber.picks.filter.language', all: 'linkgrabber.picks.filter.all_languages' }
}

/** A filter is offered only where the page has more than one value for it. */
const filterSelects = computed(() => FILTERED
  .map(name => ({ name, values: attributeValues(props.page.entries, name) }))
  .filter(({ values }) => values.length > 1)
  .map(({ name, values }) => ({
    name,
    label: t(FILTER_KEYS[name].label),
    items: [
      { label: t(FILTER_KEYS[name].all), value: ALL },
      ...values.map(value => ({ label: valueLabel(name, value), value }))
    ]
  })))

function valueLabel(name: FilteredAttribute, value: string): string {
  if (name === 'season') return t('linkgrabber.picks.season', { season: value })
  if (name === 'episode') return value === NO_EPISODE ? t('linkgrabber.picks.season_pack') : t('linkgrabber.picks.episode', { episode: value })
  return value
}

function setFilter(name: FilteredAttribute, value: string): void {
  filters.value = { ...filters.value, [name]: value }
}

/** Checked, unchecked or partly, over what can be ticked among `entries`. */
function stateOf(entries: readonly CollectorPickEntry[]): boolean | 'indeterminate' {
  const open = entries.filter(selectable)
  const count = open.filter(entry => picked.value.has(entry.index)).length
  if (!count) return false
  return count === open.length ? true : 'indeterminate'
}

function setMany(entries: readonly CollectorPickEntry[], on: boolean): void {
  const next = new Set(picked.value)
  for (const entry of entries.filter(selectable)) {
    if (on) next.add(entry.index)
    else next.delete(entry.index)
  }
  picked.value = next
}

function toggle(entry: CollectorPickEntry, on: boolean): void {
  setMany([entry], on)
}

/** The attributes a row shows beside the episode: resolution, language and hoster, where read. */
function shownAttributes(entry: CollectorPickEntry): string[] {
  return ['resolution', 'language', 'hoster']
    .map(name => entry.attributes[name])
    .filter((value): value is string => Boolean(value))
}

function seasonLabel(group: SeasonGroup): string {
  return group.season === null ? t('linkgrabber.picks.no_season') : t('linkgrabber.picks.season', { season: group.season })
}

function fetchLinks(): void {
  const entries = chosen.value.map(entry => entry.index)
  if (!entries.length) return
  emit('resolve', entries)
  picked.value = new Set()
}

function takeAll(): void {
  const entries = open.value.map(entry => entry.index)
  if (!entries.length) return
  emit('resolve', entries)
  picked.value = new Set()
}

/** The badge an entry's state gets; `pending` without a reason gets none. */
function stateBadge(entry: CollectorPickEntry): { text: string, color: 'neutral' | 'primary' | 'warning' | 'success' | 'error', title?: string | undefined } | null {
  switch (entry.state) {
    case 'queued':
      return { text: t('linkgrabber.picks.state.queued'), color: 'neutral' }
    case 'resolving':
      return { text: t('linkgrabber.picks.state.resolving'), color: 'primary' }
    case 'captcha':
      return { text: t('linkgrabber.picks.state.captcha'), color: 'warning' }
    case 'done':
      return { text: t('linkgrabber.picks.state.done', { count: entry.links }, entry.links), color: 'success' }
    case 'failed':
      return { text: t('linkgrabber.picks.state.failed'), color: 'error', title: entry.code ? t(`server.codes.${entry.code}`) : undefined }
    default:
      return entry.code
        ? { text: t('linkgrabber.picks.state.again'), color: 'neutral', title: t(`server.codes.${entry.code}`) }
        : null
  }
}
</script>

<template>
  <section class="border border-muted" :aria-label="title" data-testid="pick-page">
    <header class="flex flex-wrap items-center gap-2 border-b border-muted px-3 py-2">
      <div class="min-w-0 flex-1">
        <h3 class="truncate text-sm font-medium" :title="title">{{ title }}</h3>
        <p class="truncate text-xs text-muted">{{ t('linkgrabber.picks.source', { rule: props.page.rule, count: props.page.entries.length }, props.page.entries.length) }}</p>
      </div>
      <UBadge v-if="props.page.total" class="numeric" :color="props.page.running ? 'primary' : 'neutral'" variant="subtle" role="status" data-testid="pick-progress">
        {{ t('linkgrabber.picks.progress', { done: props.page.finished, total: props.page.total }) }}
      </UBadge>
      <UBadge v-if="props.page.waiting_for_captcha" color="warning" variant="subtle" icon="i-lucide-shield-question" data-testid="pick-captcha">
        {{ t('linkgrabber.picks.waiting_for_captcha') }}
      </UBadge>
      <UButton v-if="props.page.running" icon="i-lucide-circle-stop" size="sm" color="neutral" variant="outline" :label="t('linkgrabber.picks.cancel')" :disabled="props.busy" @click="emit('cancel')" />
      <UButton icon="i-lucide-trash-2" size="sm" color="error" variant="ghost" :aria-label="t('linkgrabber.picks.discard')" :title="t('linkgrabber.picks.discard')" :disabled="props.busy" @click="emit('discard')" />
    </header>
    <div class="flex flex-wrap items-center gap-2 px-3 py-2">
      <UCheckbox
        :model-value="stateOf(shown)"
        :label="t('linkgrabber.picks.pick_shown', { count: shown.length }, shown.length)"
        :disabled="!shown.some(selectable)"
        @update:model-value="(value: boolean | 'indeterminate') => setMany(shown, value === true)"
      />
      <USelect
        v-for="select in filterSelects"
        :key="select.name"
        :model-value="filters[select.name]"
        :items="select.items"
        value-key="value"
        class="w-40"
        :aria-label="select.label"
        @update:model-value="(value: string) => setFilter(select.name, value)"
      />
    </div>
    <p v-if="!shown.length" class="px-3 pb-2 text-xs text-muted">{{ t('linkgrabber.picks.none_shown') }}</p>
    <div v-for="group in groups" :key="group.season ?? 'none'" class="border-t border-muted">
      <div class="flex items-center gap-2 bg-elevated/50 px-3 py-1">
        <UCheckbox
          :model-value="stateOf(group.entries)"
          :label="seasonLabel(group)"
          :disabled="!group.entries.some(selectable)"
          @update:model-value="(value: boolean | 'indeterminate') => setMany(group.entries, value === true)"
        />
        <span class="numeric text-xs text-muted">{{ t('linkgrabber.picks.releases', { count: group.entries.length }, group.entries.length) }}</span>
      </div>
      <ul :aria-label="seasonLabel(group)">
        <li v-for="entry in group.entries" :key="entry.index" class="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1">
          <UCheckbox
            class="min-w-0 flex-1"
            :model-value="picked.has(entry.index) && selectable(entry)"
            :label="entry.label ?? t('linkgrabber.picks.entry', { index: entry.index + 1 })"
            :disabled="!selectable(entry)"
            :ui="{ label: 'truncate font-mono text-xs' }"
            @update:model-value="(value: boolean | 'indeterminate') => toggle(entry, value === true)"
          />
          <UBadge color="neutral" variant="outline" size="sm">
            {{ entry.attributes.episode !== undefined ? t('linkgrabber.picks.episode', { episode: entry.attributes.episode }) : t('linkgrabber.picks.season_pack') }}
          </UBadge>
          <UBadge v-for="(value, position) in shownAttributes(entry)" :key="position" color="neutral" variant="subtle" size="sm">
            {{ value }}
          </UBadge>
          <UBadge v-if="stateBadge(entry)" :color="stateBadge(entry)?.color" variant="subtle" size="sm" :title="stateBadge(entry)?.title" :data-state="entry.state">
            {{ stateBadge(entry)?.text }}
          </UBadge>
        </li>
      </ul>
    </div>
    <footer class="flex flex-wrap items-center justify-end gap-2 border-t border-muted px-3 py-2">
      <span class="text-xs text-muted">{{ t('linkgrabber.picks.captcha_hint') }}</span>
      <UButton icon="i-lucide-list-checks" size="sm" color="neutral" variant="outline" :label="t('linkgrabber.picks.take_all', { count: open.length }, open.length)" :disabled="!open.length || props.busy" data-testid="pick-take-all" @click="takeAll" />
      <UButton icon="i-lucide-download" size="sm" :label="t('linkgrabber.picks.fetch', { count: chosen.length }, chosen.length)" :disabled="!chosen.length" :loading="props.busy" data-testid="pick-fetch" @click="fetchLinks" />
    </footer>
  </section>
</template>
