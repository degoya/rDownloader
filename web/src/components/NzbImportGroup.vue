<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Category, DownloadPriority, NzbFileStatus, NzbImport, PostprocessStep } from '@/api/types'
import PostprocessSteps from '@/components/PostprocessSteps.vue'
import { formatBytes, priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const { t } = useI18n()
const PRIORITY_ITEMS = computed(() => priorityItems())

const props = defineProps<{
  item: NzbImport
  categories: Category[]
  selected: boolean
  enqueuing: boolean
  deleting: boolean
  /** True while this row is the one being dragged, so it dims like a package does. */
  dragging: boolean
}>()
const emit = defineEmits<{
  select: [id: string, selected: boolean]
  category: [id: string, categoryId: string | null]
  priority: [id: string, priority: DownloadPriority]
  enqueue: [id: string]
  /** Same enqueue, but every download of the package starts paused (RD-107-09). */
  enqueuePaused: [id: string]
  remove: [id: string]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the import up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
}>()

/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('linkgrabber.nzb.drag_hint')} — ${t('common.a11y.reorder_keys')}`)

const open = ref(false)
const pending = ref(false)
const files = ref<NzbFileStatus[]>([])
const steps = ref<PostprocessStep[]>([])

const failed = computed(() => props.item.state === 'failed')
const stateColor = computed<'success' | 'error' | 'warning'>(() => props.item.duplicate ? 'warning' : failed.value ? 'error' : 'success')
/** Online/offline equivalent: a parsed import is reachable, a failed one is not. */
const stateLabel = computed(() => props.item.duplicate
  ? t('linkgrabber.nzb.duplicate')
  : failed.value ? t('linkgrabber.nzb.state.failed') : t('linkgrabber.nzb.available'))
const categoryItems = computed(() => [
  { label: t('linkgrabber.package.default_category'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const categoryModel = computed({
  get: () => props.item.category_id ?? NO_SELECTION,
  set: (value: string) => emit('category', props.item.id, value === NO_SELECTION ? null : value)
})
const priorityModel = computed({
  get: () => props.item.priority ?? 'normal',
  set: (value: DownloadPriority) => emit('priority', props.item.id, value)
})

async function toggle(): Promise<void> {
  if (open.value) {
    open.value = false
    return
  }
  if (!files.value.length) {
    pending.value = true
    const [fileResponse, stepResponse] = await Promise.all([
      api.GET('/api/v1/nzb/imports/{id}/files', { params: { path: { id: props.item.id } } }),
      api.GET('/api/v1/nzb/imports/{id}/postprocess', { params: { path: { id: props.item.id } } })
    ])
    pending.value = false
    files.value = fileResponse.data ?? []
    steps.value = stepResponse.data ?? []
  }
  open.value = true
}

function completedSegments(file: NzbFileStatus): number {
  return file.segments.filter(segment => segment.state === 'completed').length
}
</script>

<template>
  <section
    class="border bg-elevated transition"
    :class="[props.selected ? 'border-primary' : 'border-muted', props.dragging ? 'opacity-50' : '']"
    @dragover.prevent
    @drop.prevent="emit('drop', props.item.id)"
  >
    <header class="flex items-center gap-2 px-2 py-1.5" :class="open ? 'border-b border-muted' : ''">
      <!--
        The grip replaces the file-archive icon that used to lead this row. Both kinds of entry
        share one manual order now, so both lead with the same cell; what the icon said is said
        again by the "NZB" badge further along the row, so nothing was lost with it.
      -->
      <button
        type="button"
        class="cursor-grab select-none text-muted"
        data-row-handle
        draggable="true"
        :title="dragTitle"
        :aria-label="dragTitle"
        @dragstart.stop="emit('dragstart', props.item.id)"
        @keydown.up.prevent="emit('move', props.item.id, -1)"
        @keydown.down.prevent="emit('move', props.item.id, 1)"
      >
        <UIcon name="i-lucide-grip-vertical" class="size-4" />
      </button>
      <UCheckbox :model-value="props.selected" :aria-label="t('linkgrabber.nzb.select')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', props.item.id, value === true)" />
      <UButton :icon="open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" size="xs" color="neutral" variant="ghost" :loading="pending" :aria-label="open ? t('linkgrabber.nzb.hide_files') : t('linkgrabber.nzb.show_files')" @click="toggle" />
      <div class="flex min-w-0 flex-1 items-center gap-3">
        <p class="min-w-0 flex-1 truncate text-left text-sm font-semibold text-highlighted" :title="props.item.name">{{ props.item.name }}</p>
        <span class="numeric hidden shrink-0 text-xs text-muted sm:block">
          {{ t('common.units.file', { count: props.item.file_count }, props.item.file_count) }}
          · {{ t('linkgrabber.nzb.segments', { count: props.item.segment_count }, props.item.segment_count) }}
        </span>
        <span class="numeric hidden w-24 shrink-0 text-right text-xs text-muted lg:block">{{ formatBytes(props.item.total_bytes) }}</span>
      </div>
      <span v-if="props.item.has_password" class="flex shrink-0 items-center gap-1 text-warning" :title="t('linkgrabber.nzb.password_detected')">
        <UIcon name="i-lucide-key-round" class="size-4" />
        <span v-if="props.item.password" class="max-w-32 truncate font-mono text-xs">{{ props.item.password }}</span>
      </span>
      <UBadge color="primary" variant="outline" size="sm" class="shrink-0" :title="t('linkgrabber.nzb.badge')">NZB</UBadge>
      <UBadge :color="stateColor" variant="subtle" size="sm" class="shrink-0">{{ stateLabel }}</UBadge>
      <USelect v-model="categoryModel" :items="categoryItems" value-key="value" size="xs" class="w-36" :aria-label="t('linkgrabber.package.category')" />
      <USelect v-model="priorityModel" :items="PRIORITY_ITEMS" value-key="value" size="xs" class="w-24" :aria-label="t('linkgrabber.package.priority')" />
      <UButton icon="i-lucide-arrow-down-to-line" :label="t('linkgrabber.actions.enqueue')" size="xs" color="primary" variant="soft" :disabled="props.item.duplicate || failed" :loading="props.enqueuing" @click="emit('enqueue', props.item.id)" />
      <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :title="t('linkgrabber.nzb.enqueue_paused_hint')" size="xs" color="neutral" variant="outline" :disabled="props.item.duplicate || failed" :loading="props.enqueuing" @click="emit('enqueuePaused', props.item.id)" />
      <UButton icon="i-lucide-trash-2" size="xs" color="error" variant="ghost" :aria-label="t('linkgrabber.actions.delete_nzb')" :loading="props.deleting" @click="emit('remove', props.item.id)" />
    </header>
    <UAlert v-if="props.item.error" class="mx-2 my-2" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="props.item.error" />
    <div v-if="open" class="divide-y divide-muted">
      <div v-for="file in files" :key="file.id" class="flex items-center gap-2 px-2 py-1.5 transition hover:bg-elevated/60">
        <span class="text-muted"><UIcon name="i-lucide-file" class="size-4" /></span>
        <div class="flex min-w-0 flex-1 items-center gap-3">
          <p class="min-w-0 flex-1 truncate text-sm text-highlighted" :title="file.subject">{{ file.assembly_name || file.subject }}</p>
          <UBadge color="neutral" variant="outline" size="sm" class="hidden shrink-0 font-mono md:inline-flex">{{ file.groups[0] ?? 'usenet' }}</UBadge>
          <UBadge :color="completedSegments(file) === file.segments.length ? 'success' : 'neutral'" variant="subtle" size="sm" class="numeric shrink-0" :title="t('linkgrabber.nzb.segments', { count: file.segments.length }, file.segments.length)">{{ completedSegments(file) }}/{{ file.segments.length }}</UBadge>
          <span class="numeric hidden w-24 shrink-0 text-right text-xs text-muted lg:block">{{ formatBytes(file.total_bytes) }}</span>
        </div>
      </div>
      <div v-if="steps.length" class="px-2 py-2"><PostprocessSteps :steps="steps" /></div>
    </div>
  </section>
</template>
