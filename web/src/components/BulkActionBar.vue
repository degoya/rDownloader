<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, DownloadPriority, PostprocessLevel } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { INHERIT_LEVEL, postprocessLevelItems, priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const PRIORITY_ITEMS = computed(() => priorityItems())
const LEVEL_ITEMS = computed(() => postprocessLevelItems())
const { t } = useI18n()
/**
 * One line of icons (RD-1230-02, owner 2026-10-09): every action is an icon named by its
 * `aria-label` and `title`, the two red ones with a count show it beside the icon. The shared
 * actions come in `SHARED_BULK_ACTIONS`' order in both views, each marked `data-bulk-action`;
 * a view's own actions go into the slots, between export and remove.
 */
const props = defineProps<{
  count: number
  categories: Category[]
  busy?: boolean
  /** Show start/pause (downloader) instead of enqueue (LinkGrabber). */
  transferActions?: boolean
  /** What the ticked rows are, e.g. "3 files in 2 packages": the badge's tooltip. */
  detail?: string
  /** Nothing in the selection can be exported (the LinkGrabber's NZB imports alone). */
  exportDisabled?: boolean
  /** Category/priority act on whole packages; disable when the selection is partial. */
  packageActionsDisabled?: boolean
  /**
   * The postprocess level exists on collector packages only — NZB imports carry no level, so
   * it is disabled rather than silently skipped for a pure NZB selection.
   */
  levelDisabled?: boolean
  /** Extract only makes sense for finished archives; `false` disables the action. */
  canExtract?: boolean
}>()
const emit = defineEmits<{
  category: [id: string | null]
  priority: [priority: DownloadPriority]
  /** null = inherit the category/global level */
  postprocess: [level: PostprocessLevel | null]
  resume: []
  pause: []
  cancel: []
  extract: []
  enqueue: []
  /** Same enqueue, but every download starts paused (RD-107-09). */
  enqueuePaused: []
  remove: []
  /** Jump to the first selected row. */
  reveal: []
  export: []
  clear: []
}>()

const categoryChoice = ref(NO_SELECTION)
const priorityChoice = ref<DownloadPriority | ''>('')
const levelChoice = ref('')

/**
 * The three pulldowns apply as soon as they change — there is no confirm button.
 *
 * `@update:model-value` rather than a watcher on purpose: it fires only for a real choice by the
 * user, so resetting a control from code never triggers an update of the whole selection.
 */
function applyCategory(value: string): void {
  categoryChoice.value = value
  emit('category', value === NO_SELECTION ? null : value)
}

function applyPriority(value: DownloadPriority): void {
  priorityChoice.value = value
  emit('priority', value)
}

function applyLevel(value: string): void {
  levelChoice.value = value
  emit('postprocess', value === INHERIT_LEVEL ? null : value as PostprocessLevel)
}
</script>

<template>
  <!--
    The X stands outside the wrapping group, so it never leaves the window however much the group
    holds; the group wraps where the bar is narrow (RD-1220-03). From 1440 px of window it is one
    line (RD-1230-02).
  -->
  <div class="sticky top-0 z-10 flex items-center gap-2 border border-primary/40 bg-elevated p-3" data-testid="bulk-action-bar">
    <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2" data-testid="bulk-action-group">
      <UBadge color="primary" variant="solid" class="numeric" :title="props.detail">{{ t('common.selection.count', { count: props.count }) }}</UBadge>
      <SearchableSelect
        :model-value="categoryChoice"
        :items="[{ label: t('downloads.bulk.default_category'), value: NO_SELECTION }, ...props.categories.map(category => ({ label: category.name, value: category.id }))]"
        size="sm"
        class="w-40"
        :disabled="props.packageActionsDisabled || props.busy"
        :title="props.packageActionsDisabled ? t('downloads.bulk.category_whole_packages') : undefined"
        :aria-label="t('downloads.bulk.category_aria')"
        @update:model-value="applyCategory"
      />
      <USelect :model-value="priorityChoice" :items="PRIORITY_ITEMS" value-key="value" size="sm" class="w-28" :placeholder="t('downloads.bulk.priority_placeholder')" :disabled="props.packageActionsDisabled || props.busy" :title="props.packageActionsDisabled ? t('downloads.bulk.priority_whole_packages') : undefined" :aria-label="t('downloads.bulk.priority_aria')" @update:model-value="applyPriority" />
      <USelect :model-value="levelChoice" :items="LEVEL_ITEMS" value-key="value" size="sm" class="w-32" :placeholder="t('downloads.bulk.level_placeholder')" :disabled="props.packageActionsDisabled || props.levelDisabled || props.busy" :title="props.levelDisabled ? t('downloads.bulk.level_packages_only') : props.packageActionsDisabled ? t('downloads.bulk.level_whole_packages') : undefined" :aria-label="t('downloads.bulk.level_aria')" @update:model-value="applyLevel" />
      <div class="ms-auto flex flex-wrap items-center gap-1">
        <!-- Start and pause: the queue's play and pause, or the LinkGrabber's enqueue pair. -->
        <template v-if="props.transferActions">
          <UButton size="sm" color="primary" variant="soft" icon="i-lucide-play" :aria-label="t('common.actions.start')" :title="t('common.actions.start')" :loading="props.busy" data-bulk-action="start" @click="emit('resume')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-pause" :aria-label="t('common.actions.pause')" :title="t('common.actions.pause')" :loading="props.busy" data-bulk-action="pause" @click="emit('pause')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-square" :aria-label="t('common.actions.cancel')" :title="t('common.actions.cancel')" :loading="props.busy" @click="emit('cancel')" />
        </template>
        <template v-else>
          <UButton size="sm" color="primary" variant="soft" icon="i-lucide-arrow-down-to-line" :aria-label="t('downloads.bulk.enqueue')" :title="t('downloads.bulk.enqueue')" :loading="props.busy" data-bulk-action="start" @click="emit('enqueue')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-pause" :aria-label="t('downloads.bulk.enqueue_paused')" :title="t('downloads.bulk.enqueue_paused_hint')" :loading="props.busy" data-bulk-action="pause" @click="emit('enqueuePaused')" />
        </template>
        <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-crosshair" :aria-label="t('common.actions.reveal')" :title="t('common.actions.reveal')" data-bulk-action="reveal" @click="emit('reveal')" />
        <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-file-down" :aria-label="t('common.export.action')" :title="t('common.export.action')" :disabled="props.exportDisabled" data-bulk-action="export" data-testid="bulk-export" @click="emit('export')" />
        <UButton v-if="props.transferActions" size="sm" color="neutral" variant="outline" icon="i-lucide-package-open" :aria-label="t('common.actions.extract')" :title="props.canExtract === false ? t('downloads.bulk.extract_no_archive') : t('common.actions.extract')" :disabled="props.canExtract === false" :loading="props.busy" @click="emit('extract')" />
        <!-- The view's own actions, then its red ones with a count (icon and figure, the sentence in the tooltip). -->
        <slot />
        <slot name="danger" />
        <UButton size="sm" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.remove')" :title="t('common.actions.remove')" :loading="props.busy" data-bulk-action="remove" @click="emit('remove')" />
      </div>
    </div>
    <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-x" class="shrink-0" :aria-label="t('common.actions.clear_selection')" :title="t('common.actions.clear_selection')" data-testid="bulk-clear" @click="emit('clear')" />
  </div>
</template>
