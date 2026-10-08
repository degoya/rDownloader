<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { QUEUE_COLUMN_LIMITS, QUEUE_COLUMNS, QUEUE_VIEW_COLUMNS, type QueueColumn, type QueueColumnsView } from '@/composables/useQueueColumns'
import type { QueueSort, QueueSortColumn } from '@/composables/useQueueSort'

/**
 * The column header above a list on the queue grid (RD-191-11).
 *
 * It is one more `.queue-row` with the same nine named cells, so its labels sit over the cells
 * they name at every tier and a column the tier hides takes its label with it. The start edge
 * of each data column carries a resize handle; Nuxt UI has none for a grid that is not a
 * `UTable`, so the handle is a small element of its own with the separator semantics a screen
 * reader and a keyboard need: the arrow keys move the edge (left widens, as a drag to the left
 * does), Shift moves it further, Enter and a double click put the column back. The widths
 * themselves belong to `useQueueColumns`; this only reports what was asked for.
 *
 * The two lists share the grid, not what is in it, so each names its own cells: a LinkGrabber
 * link has its link state where a download has its state, the hoster or variant where a
 * download has its category and account, and nothing in the progress cell — which is drawn
 * empty there, without a label or a handle (RD-1101-08).
 */
const props = defineProps<{
  widths: Record<QueueColumn, number>
  /** Which list this heads; it decides the labels and which columns can be resized. */
  view: QueueColumnsView
  /** The list below scrolls inside its own viewport; reserve the same scrollbar gutter. */
  gutter?: boolean
  /** Any column off its default; enables "reset all". */
  customized?: boolean
  /**
   * The labels sort the list for the eye (RD-1190-16): `null` while the queue order is shown.
   * Left out, the labels are plain text.
   */
  sort?: QueueSort | null
}>()

const emit = defineEmits<{
  resize: [column: QueueColumn, width: number]
  reset: [column: QueueColumn]
  resetAll: []
  /** A label was clicked: sort by it, turn the direction, or go back to the queue order. */
  sort: [column: QueueSortColumn]
}>()

const { t } = useI18n()

/** The catalogue key of each cell's label, per list; a cell the list leaves empty has none. */
const LABEL_KEYS: Readonly<Record<QueueColumnsView, Partial<Record<'name' | QueueColumn, string>>>> = {
  downloads: {
    name: 'common.queue_columns.name',
    state: 'common.queue_columns.state',
    progress: 'common.queue_columns.progress',
    size: 'common.queue_columns.size',
    meta: 'common.queue_columns.meta_downloads'
  },
  linkgrabber: {
    name: 'common.queue_columns.name',
    state: 'common.queue_columns.state_linkgrabber',
    size: 'common.queue_columns.size',
    meta: 'common.queue_columns.meta_linkgrabber'
  }
}

const labels = computed<Partial<Record<'name' | QueueColumn, string>>>(() => Object.fromEntries(
  Object.entries(LABEL_KEYS[props.view]).map(([cell, key]) => [cell, t(key)])
))

const resizable = computed(() => new Set(QUEUE_VIEW_COLUMNS[props.view]))
const sortable = computed(() => props.sort !== undefined)

function sortIcon(column: QueueSortColumn): string {
  if (props.sort?.column !== column) return 'i-lucide-arrow-up-down'
  return props.sort.direction === 'asc' ? 'i-lucide-arrow-up' : 'i-lucide-arrow-down'
}

/** What a click does next, said on the button: sort, turn, or back to the queue order. */
function sortLabel(column: QueueSortColumn): string {
  const name = labels.value[column] ?? ''
  if (props.sort?.column !== column) return t('common.queue_columns.sort', { column: name })
  return t(props.sort.direction === 'asc' ? 'common.queue_columns.sorted_asc' : 'common.queue_columns.sorted_desc', { column: name })
}

/** One label as a sort button: the label, the direction glyph, and what a click does. */
function sortButton(column: QueueSortColumn) {
  return {
    label: labels.value[column],
    trailingIcon: sortIcon(column),
    size: 'xs' as const,
    color: (props.sort?.column === column ? 'primary' : 'neutral') as 'primary' | 'neutral',
    variant: 'link' as const,
    class: 'max-w-full p-0 font-medium',
    ui: { label: 'truncate', trailingIcon: 'size-3' },
    'aria-label': sortLabel(column),
    title: sortLabel(column),
    'data-sort-column': column
  }
}

const STEP = 8
const BIG_STEP = 32

let drag: { column: QueueColumn, pointerId: number, startX: number, startWidth: number } | null = null

/**
 * What the column is drawn at. The grid may hold a widened column below its stored width to
 * keep the name readable, so a drag starts from the edge the viewer sees, not from the figure.
 */
function shownWidth(column: QueueColumn, handle: HTMLElement): number {
  const width = handle.parentElement?.getBoundingClientRect().width ?? 0
  return width > 0 ? Math.round(width) : props.widths[column]
}

function onPointerDown(column: QueueColumn, event: PointerEvent): void {
  if (event.button !== 0) return
  const handle = event.currentTarget as HTMLElement
  handle.setPointerCapture?.(event.pointerId)
  drag = { column, pointerId: event.pointerId, startX: event.clientX, startWidth: shownWidth(column, handle) }
  event.preventDefault()
}

function onPointerMove(event: PointerEvent): void {
  if (!drag || event.pointerId !== drag.pointerId) return
  emit('resize', drag.column, drag.startWidth + drag.startX - event.clientX)
}

function onPointerEnd(event: PointerEvent): void {
  if (!drag || event.pointerId !== drag.pointerId) return
  ;(event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId)
  drag = null
}

function onKeydown(column: QueueColumn, event: KeyboardEvent): void {
  const handle = event.currentTarget as HTMLElement
  const step = event.shiftKey ? BIG_STEP : STEP
  if (event.key === 'ArrowLeft') emit('resize', column, shownWidth(column, handle) + step)
  else if (event.key === 'ArrowRight') emit('resize', column, shownWidth(column, handle) - step)
  else if (event.key === 'Enter') emit('reset', column)
  else return
  event.preventDefault()
}

const menu = computed(() => [[
  { label: t('common.queue_columns.reset_all'), icon: 'i-lucide-rotate-ccw', disabled: !props.customized, onSelect: () => emit('resetAll') }
]])
</script>

<template>
  <div class="queue-head" :class="props.gutter ? 'overflow-hidden [scrollbar-gutter:stable]' : ''" role="group" :aria-label="t('common.queue_columns.aria')" data-testid="queue-column-header">
    <div class="queue-row border-x border-transparent px-2 text-xs font-medium text-muted">
      <span class="queue-cell-handle" />
      <span class="queue-cell-select" />
      <span class="queue-cell-expand" />
      <span class="queue-cell-name truncate">
        <UButton v-if="sortable" v-bind="sortButton('name')" @click="emit('sort', 'name')" />
        <template v-else>{{ labels.name }}</template>
      </span>
      <div v-for="column in QUEUE_COLUMNS" :key="column" class="relative min-w-0 items-center" :class="[`queue-cell-${column}`, column === 'size' ? 'text-right' : '']">
        <UButton v-if="labels[column] && sortable" v-bind="sortButton(column)" @click="emit('sort', column)" />
        <span v-else-if="labels[column]" class="block truncate">{{ labels[column] }}</span>
        <div
          v-if="resizable.has(column)"
          role="separator"
          tabindex="0"
          aria-orientation="vertical"
          :aria-valuenow="props.widths[column]"
          :aria-valuemin="QUEUE_COLUMN_LIMITS[column].min"
          :aria-valuemax="QUEUE_COLUMN_LIMITS[column].max"
          :aria-label="t('common.queue_columns.resize', { column: labels[column] })"
          :title="t('common.queue_columns.resize_hint')"
          :data-column="column"
          class="group/edge absolute inset-y-0 -left-2 z-10 flex w-2 cursor-col-resize touch-none justify-center rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
          @pointerdown="onPointerDown(column, $event)"
          @pointermove="onPointerMove"
          @pointerup="onPointerEnd"
          @pointercancel="onPointerEnd"
          @dblclick="emit('reset', column)"
          @keydown="onKeydown(column, $event)"
        >
          <span class="h-full w-px bg-accented transition group-hover/edge:bg-primary group-focus-visible/edge:bg-primary" />
        </div>
      </div>
      <div class="queue-cell-actions flex items-center justify-end">
        <UDropdownMenu :items="menu" :content="{ align: 'end' }">
          <UButton icon="i-lucide-columns-3" size="xs" color="neutral" variant="ghost" :aria-label="t('common.queue_columns.menu')" :title="t('common.queue_columns.menu')" data-testid="queue-columns-menu" />
        </UDropdownMenu>
      </div>
    </div>
  </div>
</template>
