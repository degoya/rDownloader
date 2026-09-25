<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, CollectorPackage, DownloadPriority, LinkCandidate } from '@/api/types'
import { priorityItems, formatBytes } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'
import { isEnqueueable, isUnverified } from '@/utils/candidateState'

const { t } = useI18n()
const PRIORITY_ITEMS = computed(() => priorityItems())

/**
 * The header row of one collected package.
 *
 * Until RD-106-12 this component rendered the package's links as well. The LinkGrabber now
 * flattens itself into one stream of rows so the list can be virtualized, so the links are
 * siblings of this row rather than children of it, and `open` arrives as a prop.
 */
const props = defineProps<{
  package: CollectorPackage
  /** The package's links as the active filter shows them; read for the counters and the total. */
  candidates: LinkCandidate[]
  categories: Category[]
  selectedIds: Set<string>
  enqueuingIds: Set<string>
  dragging: boolean
  /** Whether the view is currently rendering this package's link rows below the header. */
  open: boolean
}>()
const emit = defineEmits<{
  select: [ids: string[], selected: boolean]
  category: [id: string, categoryId: string | null]
  priority: [id: string, priority: DownloadPriority]
  rename: [id: string]
  enqueue: [id: string]
  /** Same enqueue, but every download of the package starts paused (RD-107-09). */
  enqueuePaused: [id: string]
  remove: [id: string]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the package up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
  /** The chevron was used; the view decides whether the link rows are in the stream. */
  toggle: [id: string]
}>()
/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('linkgrabber.package.drag_hint')} — ${t('common.a11y.reorder_keys')}`)

const selectable = computed(() => props.candidates.filter(c => isEnqueueable(c.state)).map(c => c.id))
const allSelected = computed(() => selectable.value.length > 0 && selectable.value.every(id => props.selectedIds.has(id)))
const someSelected = computed(() => selectable.value.some(id => props.selectedIds.has(id)))
const online = computed(() => props.candidates.filter(c => c.state === 'online').length)
const checking = computed(() => props.candidates.filter(c => c.state === 'checking').length)
const offline = computed(() => props.candidates.filter(c => c.state === 'offline').length)
// Split out of the offline count: a check that never got an answer is not a missing file, and
// showing both as "offline" is what made an account problem read like a dead link.
const unverified = computed(() => props.candidates.filter(c => isUnverified(c.state)).length)
const total = computed(() => props.candidates.reduce((sum, c) => sum + BigInt(c.size ?? '0'), 0n))
const busy = computed(() => props.enqueuingIds.has(props.package.id))
const categoryItems = computed(() => [
  { label: t('linkgrabber.package.default_category'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const categoryModel = computed({
  get: () => props.package.category_id ?? NO_SELECTION,
  set: (value: string) => emit('category', props.package.id, value === NO_SELECTION ? null : value)
})
const priorityModel = computed({
  get: () => props.package.priority,
  set: (value: DownloadPriority) => emit('priority', props.package.id, value)
})
</script>

<template>
  <!--
    The bottom border is dropped while the package is open: its link rows are siblings in the
    flattened stream now, not children, and they carry the frame onward themselves.
  -->
  <section
    class="border bg-elevated transition"
    :class="[someSelected ? 'border-primary' : 'border-muted', props.dragging ? 'opacity-50' : '', props.open ? 'border-b-0' : '']"
    @dragover.prevent
    @drop.prevent="emit('drop', props.package.id)"
  >
    <header class="flex items-center gap-2 px-2 py-1.5" :class="props.open ? 'border-b border-muted' : ''">
      <button
        type="button"
        class="cursor-grab select-none text-muted"
        data-row-handle
        draggable="true"
        :title="dragTitle"
        :aria-label="dragTitle"
        @dragstart.stop="emit('dragstart', props.package.id)"
        @keydown.up.prevent="emit('move', props.package.id, -1)"
        @keydown.down.prevent="emit('move', props.package.id, 1)"
      >
        <UIcon name="i-lucide-grip-vertical" class="size-4" />
      </button>
      <UCheckbox :model-value="allSelected ? true : someSelected ? 'indeterminate' : false" :disabled="!selectable.length" :aria-label="t('linkgrabber.package.select')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', selectable, value === true)" />
      <UButton :icon="props.open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" size="xs" color="neutral" variant="ghost" :aria-expanded="props.open" :aria-label="props.open ? t('linkgrabber.package.hide_links') : t('linkgrabber.package.show_links')" @click="emit('toggle', props.package.id)" />
      <div class="flex min-w-0 flex-1 items-center gap-3">
        <button type="button" class="min-w-0 flex-1 truncate text-left text-sm font-semibold text-highlighted hover:underline" :title="props.package.name" @click="emit('rename', props.package.id)">{{ props.package.name }}</button>
        <span class="numeric hidden shrink-0 text-xs text-muted sm:block">
          {{ t('common.units.link', { count: props.candidates.length }, props.candidates.length) }}
          <span v-if="online" class="text-success"> · {{ t('linkgrabber.package.online', { count: online }) }}</span>
          <span v-if="checking" class="text-primary"> · {{ t('linkgrabber.package.checking', { count: checking }) }}</span>
          <span v-if="offline" class="text-error"> · {{ t('linkgrabber.package.offline', { count: offline }) }}</span>
          <span v-if="unverified" class="text-warning"> · {{ t('linkgrabber.package.unverified', { count: unverified }) }}</span>
        </span>
        <span class="numeric hidden w-24 shrink-0 text-right text-xs text-muted lg:block">{{ total > 0n ? formatBytes(total) : '–' }}</span>
      </div>
      <span v-if="props.package.has_password" class="flex shrink-0 items-center gap-1 text-warning" :title="t('linkgrabber.package.password_hint')">
        <UIcon name="i-lucide-key-round" class="size-4" />
        <span v-if="props.package.password" class="max-w-32 truncate font-mono text-xs">{{ props.package.password }}</span>
      </span>
      <USelect v-model="categoryModel" :items="categoryItems" value-key="value" size="xs" class="w-36" :aria-label="t('linkgrabber.package.category')" />
      <USelect v-model="priorityModel" :items="PRIORITY_ITEMS" value-key="value" size="xs" class="w-24" :aria-label="t('linkgrabber.package.priority')" />
      <UButton icon="i-lucide-arrow-down-to-line" :label="t('linkgrabber.actions.enqueue')" size="xs" color="primary" variant="soft" :disabled="!selectable.length" :loading="busy" @click="emit('enqueue', props.package.id)" />
      <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :title="t('linkgrabber.package.enqueue_paused_hint')" size="xs" color="neutral" variant="outline" :disabled="!selectable.length" :loading="busy" @click="emit('enqueuePaused', props.package.id)" />
      <UButton icon="i-lucide-trash-2" size="xs" color="error" variant="ghost" :aria-label="t('linkgrabber.actions.delete_package')" @click="emit('remove', props.package.id)" />
    </header>
  </section>
</template>
