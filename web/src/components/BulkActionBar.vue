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
 * An action that is not self-evident keeps its label where the bar has room for it and shows its
 * icon alone where it has not; `aria-label` and `title` name it either way (RD-1220-03). Handed
 * to the slot too, so a caller's own labelled actions narrow the same way.
 */
const LABEL_UI = { label: 'hidden @min-[80rem]:inline' }

const props = defineProps<{
  count: number
  categories: Category[]
  busy?: boolean
  /** Show start/pause (downloader) instead of enqueue (LinkGrabber). */
  transferActions?: boolean
  /** Plural unit key, e.g. `common.units.file` or `common.units.link`; omitted = bare count. */
  unitKey?: string
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
  clear: []
}>()

const countLabel = computed(() => `${props.unitKey ? t(props.unitKey, { count: props.count }, props.count) : props.count} ${t('common.units.selected')}`)
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
    holds; the group wraps or narrows instead (RD-1220-03). `@container` lets a labelled action
    drop its label where the bar is narrow, as the LinkGrabber's navbar does.
  -->
  <div class="@container sticky top-0 z-10 flex items-start gap-2 border border-primary/40 bg-elevated p-3" data-testid="bulk-action-bar">
    <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2" data-testid="bulk-action-group">
      <UBadge color="primary" variant="solid" class="numeric">{{ countLabel }}</UBadge>
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
        <!-- Start, pause, stop and remove are what every player and file list shows: icons, named by `aria-label` and `title`. -->
        <template v-if="props.transferActions">
          <UButton size="sm" color="primary" variant="soft" icon="i-lucide-play" :aria-label="t('common.actions.start')" :title="t('common.actions.start')" :loading="props.busy" @click="emit('resume')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-pause" :aria-label="t('common.actions.pause')" :title="t('common.actions.pause')" :loading="props.busy" @click="emit('pause')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-square" :aria-label="t('common.actions.cancel')" :title="t('common.actions.cancel')" :loading="props.busy" @click="emit('cancel')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-package-open" :label="t('common.actions.extract')" :aria-label="t('common.actions.extract')" :title="props.canExtract === false ? t('downloads.bulk.extract_no_archive') : t('common.actions.extract')" :ui="LABEL_UI" :disabled="props.canExtract === false" :loading="props.busy" @click="emit('extract')" />
        </template>
        <template v-else>
          <UButton size="sm" color="primary" variant="soft" icon="i-lucide-arrow-down-to-line" :label="t('downloads.bulk.enqueue')" :loading="props.busy" @click="emit('enqueue')" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-pause" :label="t('downloads.bulk.enqueue_paused')" :title="t('downloads.bulk.enqueue_paused_hint')" :loading="props.busy" @click="emit('enqueuePaused')" />
        </template>
        <slot :label-ui="LABEL_UI" />
        <UButton size="sm" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.remove')" :title="t('common.actions.remove')" :loading="props.busy" @click="emit('remove')" />
      </div>
    </div>
    <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-x" class="shrink-0" :aria-label="t('common.actions.clear_selection')" :title="t('common.actions.clear_selection')" data-testid="bulk-clear" @click="emit('clear')" />
  </div>
</template>
