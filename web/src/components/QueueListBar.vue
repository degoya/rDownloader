<script setup lang="ts">
import { useI18n } from 'vue-i18n'

/**
 * The row right above a queue list, shared by Downloads and the LinkGrabber (RD-1230-02).
 *
 * It starts with what acts on the list's rows — select all with its count, open or close every
 * package — and stands at the list rather than in the page's toolbar, so the count is read where
 * the ticks are (owner, 2026-10-09). Then the view's own search or filters, and at the end the
 * same three in the same place in both views: export everything, the metadata switch, the count.
 * The order is `LIST_BAR_ORDER` (`utils/listLayout.ts`), each place marked `data-list-bar`.
 */
const props = defineProps<{
  /** How much of what the list shows is ticked. */
  state: 'all' | 'some' | 'none'
  count: number
  /** What the ticked rows are, e.g. "3 files in 2 packages": the tooltip of the count. */
  detail?: string
  /** The checkbox's accessible name ("Select all files"). */
  selectHint: string
  /** Nothing to select or open. */
  empty: boolean
  allOpen: boolean
  exportDisabled: boolean
  /** "2 packages · 5 files", the figures of what the list shows. */
  countText: string
}>()
const emit = defineEmits<{
  toggleAll: []
  toggleOpen: []
  exportAll: []
}>()
const showMetadata = defineModel<boolean>('showMetadata', { required: true })
const { t } = useI18n()
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5" data-testid="queue-list-bar">
    <div class="flex min-w-0 flex-auto flex-wrap items-center gap-2">
      <UCheckbox
        :model-value="props.state === 'all' ? true : props.state === 'some' ? 'indeterminate' : false"
        :disabled="props.empty"
        :label="props.count ? t('common.selection.count', { count: props.count }) : t('common.actions.select_all')"
        :aria-label="props.selectHint"
        :title="props.count ? props.detail : undefined"
        :ui="{ root: 'shrink-0', label: 'whitespace-nowrap' }"
        data-list-bar="select"
        data-testid="list-select-all"
        @update:model-value="emit('toggleAll')"
      />
      <UButton :icon="props.allOpen ? 'i-lucide-chevrons-down-up' : 'i-lucide-chevrons-up-down'" color="neutral" variant="ghost" :aria-label="t(props.allOpen ? 'common.package_groups.close_all' : 'common.package_groups.open_all')" :title="t(props.allOpen ? 'common.package_groups.close_all' : 'common.package_groups.open_all')" :disabled="props.empty" data-list-bar="expand" data-testid="packages-open-toggle" @click="emit('toggleOpen')" />
      <div class="flex min-w-0 flex-wrap items-center gap-2" data-list-bar="filters">
        <slot name="filters" />
      </div>
    </div>
    <div class="ms-auto flex flex-wrap items-center gap-2">
      <UButton icon="i-lucide-file-down" color="neutral" variant="ghost" :aria-label="t('common.export.action_all')" :title="t('common.export.action_all')" :disabled="props.exportDisabled" data-list-bar="export" data-testid="list-export-all" @click="emit('exportAll')" />
      <USwitch v-model="showMetadata" size="sm" :label="t('common.enrichment.show')" :title="t('common.enrichment.show_hint')" :ui="{ label: 'whitespace-nowrap' }" data-list-bar="metadata" data-testid="show-metadata" />
      <span class="numeric whitespace-nowrap text-xs text-muted" data-list-bar="count">{{ props.countText }}</span>
      <slot name="end" />
    </div>
  </div>
</template>
