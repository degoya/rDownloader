<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Category, DownloadPriority, NzbFileStatus, NzbImport, PostprocessStep } from '@/api/types'
import PostprocessSteps from '@/components/PostprocessSteps.vue'
import type { NzbHandOverTarget } from '@/composables/useNzbHandOver'
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
  /** Accounts whose provider takes NZB files (RD-191-13); without any, the action is not shown. */
  remoteTargets?: NzbHandOverTarget[]
  /** The provider this import was handed to, when it was. */
  handedOverTo?: string | null
  handingOver?: boolean
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
  /** To a provider's account instead of the queue (RD-191-13). */
  handOver: [id: string, accountId: string]
}>()

/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('linkgrabber.nzb.drag_hint')} — ${t('common.a11y.reorder_keys')}`)

const open = ref(false)
const pending = ref(false)
const files = ref<NzbFileStatus[]>([])
const steps = ref<PostprocessStep[]>([])

const failed = computed(() => props.item.state === 'failed')
/** A failed import whose group, opened, would explain nothing: no error and no failed step. */
const failedSilently = computed(() => failed.value && !props.item.error && !steps.value.some(step => step.state === 'failed'))
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
/** One entry per account whose provider takes NZB files; picking one hands this import over. */
const handOverItems = computed(() => (props.remoteTargets ?? []).map(target => ({
  label: target.label,
  icon: 'i-lucide-cloud-upload',
  onSelect: () => emit('handOver', props.item.id, target.accountId)
})))
/** Enqueueing stays possible after a hand-over; the buttons say what it would add. */
const enqueueHint = computed(() => props.handedOverTo ? t('linkgrabber.nzb.hand_over.enqueue_hint', { provider: props.handedOverTo }) : undefined)
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
    class="@container border bg-elevated transition"
    :class="[props.selected ? 'border-primary' : 'border-muted', props.dragging ? 'opacity-50' : '']"
    @dragover.prevent
    @drop.prevent="emit('drop', props.item.id)"
  >
    <!-- Wraps on its own width as the package row does; its controls are wider, so they share
         the name's line from 70 rem and the size from 80 rem. -->
    <header class="flex flex-wrap items-center gap-x-2 gap-y-1.5 px-2 py-1.5" :class="open ? 'border-b border-muted' : ''">
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
      <UButton :icon="open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" size="xs" color="neutral" variant="ghost" :loading="pending" :aria-expanded="open" :aria-label="open ? t('linkgrabber.nzb.hide_files') : t('linkgrabber.nzb.show_files')" @click="toggle" />
      <div class="flex min-w-0 shrink grow basis-[200px] items-center gap-3">
        <p class="min-w-50 flex-1 truncate text-left text-sm font-semibold text-highlighted" :title="props.item.name">{{ props.item.name }}</p>
        <span class="numeric hidden min-w-0 truncate text-xs text-muted @min-[32rem]:block">
          {{ t('common.units.file', { count: props.item.file_count }, props.item.file_count) }}
          · {{ t('linkgrabber.nzb.segments', { count: props.item.segment_count }, props.item.segment_count) }}
        </span>
        <span class="numeric hidden w-24 shrink-0 text-right text-xs text-muted @min-[40rem]:block @min-[70rem]:hidden @min-[80rem]:block">{{ formatBytes(props.item.total_bytes) }}</span>
      </div>
      <div class="ms-auto flex w-full flex-wrap items-center justify-end gap-2 @min-[70rem]:w-auto">
      <span v-if="props.item.has_password" class="flex shrink-0 items-center gap-1 text-warning" :title="t('linkgrabber.nzb.password_detected')">
        <UIcon name="i-lucide-key-round" class="size-4" />
        <span v-if="props.item.password" class="max-w-32 truncate font-mono text-xs">{{ props.item.password }}</span>
      </span>
      <UBadge color="primary" variant="outline" size="sm" class="shrink-0" :title="t('linkgrabber.nzb.badge')">NZB</UBadge>
      <!-- A failed import opens on its badge, as a failed package does: the reason is what the
           reader came for (RD-191-11). A duplicate or a usable import keeps a plain badge. -->
      <UButton
        v-if="failed && !props.item.duplicate"
        color="error"
        variant="subtle"
        size="xs"
        class="shrink-0"
        :label="stateLabel"
        :title="props.item.error || t('linkgrabber.nzb.show_files')"
        :aria-expanded="open"
        :loading="pending"
        data-testid="nzb-failed"
        @click="toggle"
      />
      <UBadge v-else :color="stateColor" variant="subtle" size="sm" class="shrink-0">{{ stateLabel }}</UBadge>
      <!-- Handed to a provider (RD-191-13): the import stays here so it is not queued twice by
           accident, and the badge leads to where the job can be watched. -->
      <UButton
        v-if="props.handedOverTo"
        :to="{ name: 'remote-jobs' }"
        icon="i-lucide-cloud"
        color="info"
        variant="subtle"
        size="xs"
        class="shrink-0"
        :label="t('linkgrabber.nzb.hand_over.badge', { provider: props.handedOverTo })"
        :title="t('linkgrabber.nzb.hand_over.badge_hint')"
        data-testid="nzb-handed-over"
      />
      <USelect v-model="categoryModel" :items="categoryItems" value-key="value" size="xs" class="w-36" :aria-label="t('linkgrabber.package.category')" />
      <USelect v-model="priorityModel" :items="PRIORITY_ITEMS" value-key="value" size="xs" class="w-24" :aria-label="t('linkgrabber.package.priority')" />
      <UButton icon="i-lucide-arrow-down-to-line" :label="t('linkgrabber.actions.enqueue')" :title="enqueueHint" size="xs" color="primary" variant="soft" :disabled="props.item.duplicate || failed" :loading="props.enqueuing" @click="emit('enqueue', props.item.id)" />
      <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :title="enqueueHint ?? t('linkgrabber.nzb.enqueue_paused_hint')" size="xs" color="neutral" variant="outline" :disabled="props.item.duplicate || failed" :loading="props.enqueuing" @click="emit('enqueuePaused', props.item.id)" />
      <UDropdownMenu v-if="handOverItems.length && !failed" :items="handOverItems">
        <UButton
          icon="i-lucide-cloud-upload"
          size="xs"
          color="neutral"
          variant="outline"
          :aria-label="t('linkgrabber.nzb.hand_over.action')"
          :title="t('linkgrabber.nzb.hand_over.hint')"
          :loading="props.handingOver"
          data-testid="nzb-hand-over"
        />
      </UDropdownMenu>
      <UButton icon="i-lucide-trash-2" size="xs" color="error" variant="ghost" :aria-label="t('linkgrabber.actions.delete_nzb')" :loading="props.deleting" @click="emit('remove', props.item.id)" />
      </div>
    </header>
    <UAlert v-if="props.item.error" class="mx-2 my-2" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="props.item.error" />
    <div v-if="open" class="divide-y divide-muted">
      <p v-if="failedSilently" class="px-2 py-2 text-sm text-error">{{ t('linkgrabber.nzb.failed_no_reason') }}</p>
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
