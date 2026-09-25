<script setup lang="ts" generic="T extends VirtualRow">
import { computed, nextTick, ref } from 'vue'

import { DEFAULT_THRESHOLD, useVirtualRows, type VirtualRow } from '@/composables/useVirtualRows'

/**
 * The one long-list building block, used by the queue and by the LinkGrabber (RD-106-12).
 *
 * It takes a flat stream of rows — the views flatten their trees into one, package header
 * followed by its children while it is open — and puts only the visible slice in the document.
 * What it guarantees beyond that:
 *
 * - **The list says how long it is.** `role="list"` with a name, and every row carries
 *   `aria-setsize` / `aria-posinset`, so "row 1400 of 3200" is still true when only thirty of
 *   them exist (`docs/accessibility.md`).
 * - **A focused row is never taken out.** The row holding focus is pinned, so the arrow-key
 *   reorder on a drag handle keeps working while the list scrolls under it.
 * - **`focusRow()` reaches a row that is not rendered**, which is what a jump to an entry and a
 *   keyboard move past the edge of the viewport both need.
 */
const props = withDefaults(defineProps<{
  rows: T[]
  /** Accessible name of the list; say what it holds and how much. */
  label: string
  /** Row count from which windowing starts; below it the list renders whole and does not scroll. */
  threshold?: number
  /** Rows the view wants kept in the document besides the focused one. */
  pinnedKeys?: string[]
  /** Height of the scroll viewport once windowing is on. */
  maxHeight?: string
}>(), {
  threshold: DEFAULT_THRESHOLD,
  pinnedKeys: () => [],
  maxHeight: '70vh'
})

const rows = computed(() => props.rows)
const threshold = computed(() => props.threshold)

const focusedKey = ref<string | null>(null)
/** A row being scrolled to, pinned until the focus has actually landed on it. */
const wantedKey = ref<string | null>(null)

const pinned = computed(() => {
  const keys = [...props.pinnedKeys]
  if (focusedKey.value) keys.push(focusedKey.value)
  if (wantedKey.value) keys.push(wantedKey.value)
  return keys
})

const { viewport, windowed, rendered, padTop, padBottom, onScroll, indexOfKey, scrollToKey } =
  useVirtualRows(rows, { threshold, pinned })

function rowKeyOf(target: EventTarget | null): string | null {
  if (!(target instanceof Element)) return null
  return target.closest('[data-row-key]')?.getAttribute('data-row-key') ?? null
}

function onFocusIn(event: FocusEvent): void {
  focusedKey.value = rowKeyOf(event.target)
}

/** Focus left the list itself; nothing to hold on to any more. */
function onFocusOut(event: FocusEvent): void {
  const next = event.relatedTarget
  if (next instanceof Node && viewport.value?.contains(next)) return
  focusedKey.value = null
}

function elementFor(key: string): HTMLElement | null {
  return viewport.value?.querySelector<HTMLElement>(`[data-row-key="${key}"]`) ?? null
}

/**
 * Brings a row into the window and puts the keyboard on it.
 *
 * The row may not be rendered yet, so it is pinned first, rendered on the next tick and only
 * then scrolled to and focused. The handle is preferred over the wrapper: that is the control
 * the arrow keys belong to, and landing on the wrapper would silently end the reorder.
 */
async function focusRow(key: string): Promise<boolean> {
  if (indexOfKey(key) < 0) return false
  wantedKey.value = key
  await nextTick()
  scrollToKey(key)
  await nextTick()
  const row = elementFor(key)
  const handle = row?.querySelector<HTMLElement>('[data-row-handle]') ?? row
  handle?.focus()
  wantedKey.value = null
  return Boolean(row)
}

/** Scrolls a row into view without taking focus away from wherever it is. */
async function revealRow(key: string): Promise<boolean> {
  if (indexOfKey(key) < 0) return false
  wantedKey.value = key
  await nextTick()
  const found = scrollToKey(key)
  await nextTick()
  wantedKey.value = null
  return found
}

defineExpose({ focusRow, revealRow, scrollToKey, windowed })
</script>

<template>
  <div
    ref="viewport"
    :class="windowed ? 'min-h-0 overflow-y-auto' : ''"
    :style="windowed ? { maxHeight: props.maxHeight } : undefined"
    @scroll="onScroll"
    @focusin="onFocusIn"
    @focusout="onFocusOut"
  >
    <div
      role="list"
      :aria-label="props.label"
      :style="{ paddingTop: `${padTop}px`, paddingBottom: `${padBottom}px` }"
    >
      <div
        v-for="entry in rendered"
        :key="entry.row.key"
        role="listitem"
        :data-row-key="entry.row.key"
        :aria-setsize="props.rows.length"
        :aria-posinset="entry.index + 1"
        :class="entry.row.class"
      >
        <slot name="row" :row="entry.row" :index="entry.index" />
      </div>
    </div>
  </div>
</template>
