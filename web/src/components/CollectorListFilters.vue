<script setup lang="ts">
import { useToast } from '@nuxt/ui/composables'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { useGrabberFacets } from '@/composables/useGrabberFacets'
import { useCollectorStore } from '@/stores/collector'
import { SORT_OPTIONS, type CollectorSort } from '@/utils/collectorSort'

/**
 * The LinkGrabber's filters in the row above its list (RD-1230-02).
 *
 * The state filter stands where Downloads has its state filter. What only the LinkGrabber has —
 * the three facets (RD-110-19), the sort and *Regroup* — waits behind one button, because they are
 * set now and then rather than looked at, and in the row they broke it onto a second line. The
 * button counts what is set inside it, so a list narrowed from behind it says so; *Clear filters*
 * stays in the row while anything filters.
 *
 * The LinkFilter (RD-1240-09) sits here too: *Show hidden* draws the links its rules hid, and
 * *Apply rules* decides the list's links again by the rules as they are now.
 */
const props = defineProps<{
  facets: ReturnType<typeof useGrabberFacets>
}>()
const emit = defineEmits<{ regroup: [] }>()
const sort = defineModel<CollectorSort>('sort', { required: true })
const descending = defineModel<boolean>('descending', { required: true })
const { t } = useI18n()

const {
  hosterFilter, qualityFilter, languageFilter, stateFilter, facetBusy, facetFilterActive,
  hosterItems, qualityItems, languageItems, stateItems, clearFilters, showFiltered, filteredCount
} = props.facets
const collector = useCollectorStore()
const toast = useToast()
const applying = ref(false)

/** Applies the LinkFilter rules to the list and says what changed. */
async function applyRules(): Promise<void> {
  applying.value = true
  const outcome = await collector.applyLinkFilters()
  applying.value = false
  if (!outcome) return
  toast.add({ title: t('linkgrabber.link_filter.applied', { hidden: outcome.hidden, shown: outcome.shown, routed: outcome.routed }), color: 'success', icon: 'i-lucide-filter' })
}

const sortItems = computed(() => SORT_OPTIONS.map(option => ({ label: t(option.labelKey), value: option.value })))
/** What is set behind the button: each facet off its "any", and a sort other than the manual order. */
const hiddenSet = computed(() => [qualityFilter.value, languageFilter.value, hosterFilter.value].filter(value => value !== 'all').length
  + (sort.value === 'manual' ? 0 : 1) + (showFiltered.value ? 1 : 0))
</script>

<template>
  <USelect v-model="stateFilter" :items="stateItems" value-key="value" class="w-36" :aria-label="t('linkgrabber.filter.state_label')" />
  <UPopover :content="{ align: 'start' }">
    <UButton icon="i-lucide-sliders-horizontal" color="neutral" variant="outline" :label="t('linkgrabber.filter.more')" :title="t('linkgrabber.filter.more_hint')" data-testid="grabber-filter-more">
      <template v-if="hiddenSet" #trailing>
        <UBadge color="primary" variant="subtle" size="sm" class="numeric">{{ hiddenSet }}</UBadge>
      </template>
    </UButton>
    <template #content>
      <div class="flex w-72 flex-col gap-2 p-3" data-testid="grabber-filter-panel">
        <!-- The three facets. Inside a mirror group they choose the member the queue will fetch;
             outside one they hide what cannot satisfy them, and they stay set for the next
             package (RD-110-19, `design.md`). -->
        <p class="text-xs text-muted">{{ t('linkgrabber.filter.facet_hint') }}</p>
        <USelect v-model="qualityFilter" :items="qualityItems" value-key="value" class="w-full" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.quality_label')" />
        <USelect v-model="languageFilter" :items="languageItems" value-key="value" class="w-full" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.language_label')" />
        <USelect v-model="hosterFilter" :items="hosterItems" value-key="value" class="w-full" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.hoster_label')" />
        <USeparator />
        <USwitch v-model="showFiltered" :label="t('linkgrabber.link_filter.show_hidden', { count: filteredCount })" data-testid="grabber-show-filtered" />
        <UButton icon="i-lucide-filter" :label="t('linkgrabber.link_filter.apply')" :title="t('linkgrabber.link_filter.apply_hint')" color="neutral" variant="ghost" class="self-start" :loading="applying" data-testid="grabber-apply-filters" @click="applyRules" />
        <USeparator />
        <div class="flex items-center gap-2">
          <USelect v-model="sort" :items="sortItems" value-key="value" class="min-w-0 flex-1" :aria-label="t('linkgrabber.sort.label')" />
          <UButton :icon="descending ? 'i-lucide-arrow-down-wide-narrow' : 'i-lucide-arrow-up-narrow-wide'" color="neutral" variant="ghost" :aria-label="descending ? t('linkgrabber.sort.descending') : t('linkgrabber.sort.ascending')" :title="descending ? t('linkgrabber.sort.descending') : t('linkgrabber.sort.ascending')" :disabled="sort === 'manual'" @click="descending = !descending" />
        </div>
        <UButton icon="i-lucide-group" :label="t('linkgrabber.actions.regroup')" color="neutral" variant="ghost" class="self-start" @click="emit('regroup')" />
      </div>
    </template>
  </UPopover>
  <UButton v-if="facetFilterActive" icon="i-lucide-filter-x" color="neutral" variant="ghost" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.clear')" :title="t('linkgrabber.filter.clear')" @click="clearFilters" />
</template>
