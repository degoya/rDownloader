<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, CollectorPackage, DownloadPriority, LinkCandidate } from '@/api/types'
import DragHandle from '@/components/DragHandle.vue'
import SearchableSelect from '@/components/SearchableSelect.vue'
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
  /** Every link of the package onto the clipboard; the view gathers them (RD-190-21). */
  copyLinks: [id: string]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the package up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
  /** The chevron was used; the view decides whether the link rows are in the stream. */
  toggle: [id: string]
  /** Every package the view shows, open or closed at once (RD-1170-01). */
  openAll: []
  closeAll: []
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
/**
 * The name the package gets in the queue: tidied by the package-name rules when the service
 * says so (RD-1140-05). Renaming still edits the name itself, which then stays as typed.
 */
const shownName = computed(() => props.package.queue_name ?? props.package.name)
const nameTitle = computed(() => props.package.queue_name
  ? t('linkgrabber.package.queue_name_hint', { name: props.package.name })
  : props.package.name)
const categoryItems = computed(() => [
  { label: t('linkgrabber.package.default_category'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const categoryModel = computed({
  get: () => props.package.category_id ?? NO_SELECTION,
  set: (value: string) => emit('category', props.package.id, value === NO_SELECTION ? null : value)
})
/** The row's menu: what acts on the list rather than on this package (RD-1170-01). */
const listActions = computed(() => [[
  { label: t('common.package_groups.open_all'), icon: 'i-lucide-chevrons-up-down', onSelect: () => emit('openAll') },
  { label: t('common.package_groups.close_all'), icon: 'i-lucide-chevrons-down-up', onSelect: () => emit('closeAll') }
]])
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
    class="@container border bg-elevated transition"
    :class="[someSelected ? 'border-primary' : 'border-muted', props.dragging ? 'opacity-50' : '', props.open ? 'border-b-0' : '']"
    @dragover.prevent
    @drop.prevent="emit('drop', props.package.id)"
  >
    <!--
      The row measures its own width (`@container`), not the window's: beside an open sidebar the
      window says "wide" while the row is not, and the name was squeezed to nothing. Below 64 rem
      the controls take a line of their own under the name; the name never goes below 200 px —
      the link count gives way first — and the size shows only where a line still has room for it.
    -->
    <header class="flex flex-wrap items-center gap-x-2 gap-y-1.5 px-2 py-1.5" :class="props.open ? 'border-b border-muted' : ''">
      <DragHandle
        :label="dragTitle"
        @dragstart="emit('dragstart', props.package.id)"
        @move="(delta: -1 | 1) => emit('move', props.package.id, delta)"
      />
      <UCheckbox :model-value="allSelected ? true : someSelected ? 'indeterminate' : false" :disabled="!selectable.length" :aria-label="t('linkgrabber.package.select')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', selectable, value === true)" />
      <UButton :icon="props.open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" size="xs" color="neutral" variant="ghost" :aria-expanded="props.open" :aria-label="props.open ? t('linkgrabber.package.hide_links') : t('linkgrabber.package.show_links')" @click="emit('toggle', props.package.id)" />
      <div class="flex min-w-0 shrink grow basis-[200px] items-center gap-3">
        <UButton variant="link" color="neutral" class="min-w-50 flex-1 p-0 text-left text-sm font-semibold text-highlighted hover:text-highlighted hover:underline" :label="shownName" :title="nameTitle" data-testid="collector-package-name" @click="emit('rename', props.package.id)" />
        <span class="numeric hidden min-w-0 truncate text-xs text-muted @min-[32rem]:block">
          {{ t('common.units.link', { count: props.candidates.length }, props.candidates.length) }}
          <span v-if="online" class="text-success"> · {{ t('linkgrabber.package.online', { count: online }) }}</span>
          <span v-if="checking" class="text-primary"> · {{ t('linkgrabber.package.checking', { count: checking }) }}</span>
          <span v-if="offline" class="text-error"> · {{ t('linkgrabber.package.offline', { count: offline }) }}</span>
          <span v-if="unverified" class="text-warning"> · {{ t('linkgrabber.package.unverified', { count: unverified }) }}</span>
        </span>
        <span class="numeric hidden w-24 shrink-0 text-right text-xs text-muted @min-[40rem]:block @min-[64rem]:hidden @min-[76rem]:block">{{ total > 0n ? formatBytes(total) : '–' }}</span>
      </div>
      <div class="ms-auto flex w-full flex-wrap items-center justify-end gap-2 @min-[64rem]:w-auto">
      <span v-if="props.package.has_password" class="flex shrink-0 items-center gap-1 text-warning" :title="t('linkgrabber.package.password_hint')">
        <UIcon name="i-lucide-key-round" class="size-4" />
        <span v-if="props.package.password" class="max-w-32 truncate font-mono text-xs">{{ props.package.password }}</span>
      </span>
      <SearchableSelect v-model="categoryModel" :items="categoryItems" size="xs" class="w-36" :aria-label="t('linkgrabber.package.category')" />
      <USelect v-model="priorityModel" :items="PRIORITY_ITEMS" value-key="value" size="xs" class="w-24" :aria-label="t('linkgrabber.package.priority')" />
      <UButton icon="i-lucide-arrow-down-to-line" :label="t('linkgrabber.actions.enqueue')" size="xs" color="primary" variant="soft" :disabled="!selectable.length" :loading="busy" @click="emit('enqueue', props.package.id)" />
      <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :title="t('linkgrabber.package.enqueue_paused_hint')" size="xs" color="neutral" variant="outline" :disabled="!selectable.length" :loading="busy" @click="emit('enqueuePaused', props.package.id)" />
      <UButton icon="i-lucide-link" size="xs" color="neutral" variant="ghost" :aria-label="t('common.actions.copy_links')" :title="t('common.actions.copy_links')" @click="emit('copyLinks', props.package.id)" />
      <UButton icon="i-lucide-trash-2" size="xs" color="error" variant="ghost" :aria-label="t('linkgrabber.actions.delete_package')" @click="emit('remove', props.package.id)" />
      <UDropdownMenu :items="listActions" :content="{ align: 'end' }">
        <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('linkgrabber.package.actions')" :title="t('linkgrabber.package.actions')" />
      </UDropdownMenu>
      </div>
    </header>
  </section>
</template>
