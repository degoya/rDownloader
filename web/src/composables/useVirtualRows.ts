import { computed, onMounted, onUnmounted, onUpdated, ref, watch, type Ref } from 'vue'

/**
 * Windowing over a flat stream of rows (RD-106-12).
 *
 * The queue and the LinkGrabber are trees, and a tree cannot be virtualized as a tree: the
 * views flatten themselves into a single sequence of rows with stable keys — package header,
 * then its children while it is open — and this composable decides which slice of that
 * sequence is in the document.
 *
 * Two properties matter more than the arithmetic:
 *
 * - **A pinned row is never removed.** Virtualization takes rows out of the DOM, and the row
 *   that is taken out while one of its buttons has focus takes the focus with it. Every drag
 *   handle in both lists is a focusable button that reorders with the arrow keys
 *   (`docs/accessibility.md`, WCAG 2.1.1 and 2.5.7), so the focused row — and any row the view
 *   pins deliberately, e.g. a jump target — is rendered wherever it sits. It is placed next to
 *   the window boundary and paid for out of the padding, so the total height stays right and
 *   the row itself stays off screen.
 * - **A short list is not windowed at all.** Below `threshold` rows the composable renders
 *   everything and the viewport keeps its natural height, so an ordinary queue looks and
 *   behaves exactly as it did before this existed.
 */

/** The least a row must say about itself: what identifies it and how tall it probably is. */
export interface VirtualRow {
  key: string
  /** Estimated height in pixels, replaced by the measured one once the row has been rendered. */
  size: number
  /** Optional class for the wrapper the list draws around the row. */
  class?: string
}

export interface RenderedRow<T extends VirtualRow> {
  row: T
  index: number
}

/** Rows below this are rendered whole; windowing costs more than it saves on a short list. */
export const DEFAULT_THRESHOLD = 60
const DEFAULT_OVERSCAN = 6
/** Stands in until the viewport has a measurable height — first paint, and jsdom, which never has one. */
const FALLBACK_VIEWPORT = 800

export interface VirtualRowsOptions {
  /** Row count from which windowing starts. */
  threshold?: Ref<number>
  /** Rows kept above and below the visible range, so a scroll does not chase the renderer. */
  overscan?: number
  /** Keys that stay in the document wherever they sit — the focused row, a jump target. */
  pinned?: Ref<string[]>
}

export function useVirtualRows<T extends VirtualRow>(rows: Ref<T[]>, options: VirtualRowsOptions = {}) {
  const viewport = ref<HTMLElement | null>(null)
  const scrollTop = ref(0)
  const viewportHeight = ref(0)
  /** Real heights, keyed by row key. An estimate is only ever a starting point. */
  const measured = ref(new Map<string, number>())
  const overscan = options.overscan ?? DEFAULT_OVERSCAN

  const threshold = computed(() => options.threshold?.value ?? DEFAULT_THRESHOLD)
  const windowed = computed(() => rows.value.length > threshold.value)

  function sizeOf(row: T): number {
    return measured.value.get(row.key) ?? row.size
  }

  /** `offsets[i]` is where row `i` starts; the last entry is the total height. */
  const offsets = computed<number[]>(() => {
    const list = rows.value
    const result: number[] = new Array<number>(list.length + 1)
    let sum = 0
    for (let index = 0; index < list.length; index += 1) {
      result[index] = sum
      const row = list[index]
      if (row) sum += sizeOf(row)
    }
    result[list.length] = sum
    return result
  })

  const totalHeight = computed(() => offsets.value[rows.value.length] ?? 0)

  /** First row whose end is past `position`. */
  function indexAt(position: number): number {
    const table = offsets.value
    let low = 0
    let high = rows.value.length - 1
    while (low < high) {
      const middle = (low + high) >> 1
      if ((table[middle + 1] ?? 0) <= position) low = middle + 1
      else high = middle
    }
    return low
  }

  const range = computed(() => {
    const count = rows.value.length
    if (!windowed.value || !count) return { start: 0, end: count - 1 }
    const height = viewportHeight.value || FALLBACK_VIEWPORT
    const start = Math.max(0, indexAt(scrollTop.value) - overscan)
    const end = Math.min(count - 1, indexAt(scrollTop.value + height) + overscan)
    return { start, end }
  })

  /** Pinned rows outside the window, split into the ones above it and the ones below. */
  const pinnedOutside = computed(() => {
    const above: RenderedRow<T>[] = []
    const below: RenderedRow<T>[] = []
    if (!windowed.value) return { above, below }
    const keys = options.pinned?.value ?? []
    if (!keys.length) return { above, below }
    const wanted = new Set(keys)
    const { start, end } = range.value
    rows.value.forEach((row, index) => {
      if (!wanted.has(row.key) || (index >= start && index <= end)) return
      if (index < start) above.push({ row, index })
      else below.push({ row, index })
    })
    return { above, below }
  })

  const rendered = computed<RenderedRow<T>[]>(() => {
    const { start, end } = range.value
    const inside: RenderedRow<T>[] = []
    for (let index = start; index <= end; index += 1) {
      const row = rows.value[index]
      if (row) inside.push({ row, index })
    }
    return [...pinnedOutside.value.above, ...inside, ...pinnedOutside.value.below]
  })

  function heightOf(entries: RenderedRow<T>[]): number {
    return entries.reduce((sum, entry) => sum + sizeOf(entry.row), 0)
  }

  /**
   * The space the rows above the window would have taken, minus the pinned ones that are
   * actually rendered up there. Padding rather than spacer elements, so the `role="list"`
   * really only contains list items.
   */
  const padTop = computed(() => {
    if (!windowed.value) return 0
    const start = offsets.value[range.value.start] ?? 0
    return Math.max(0, start - heightOf(pinnedOutside.value.above))
  })

  const padBottom = computed(() => {
    if (!windowed.value) return 0
    const after = offsets.value[range.value.end + 1] ?? totalHeight.value
    return Math.max(0, totalHeight.value - after - heightOf(pinnedOutside.value.below))
  })

  function readViewport(): void {
    const element = viewport.value
    if (!element) return
    scrollTop.value = element.scrollTop
    if (element.clientHeight > 0) viewportHeight.value = element.clientHeight
  }

  /**
   * Reads back what the rendered rows really measure.
   *
   * Estimates are wrong the moment a file name wraps or a card opens its detail, and a wrong
   * estimate shows up as a scrollbar that does not match the content. A zero is ignored: that
   * is what an element without layout reports, and overwriting an estimate with it would
   * collapse the list.
   */
  function measure(): void {
    const element = viewport.value
    if (!element || !windowed.value) return
    let changed = false
    const next = new Map(measured.value)
    for (const node of element.querySelectorAll<HTMLElement>('[data-row-key]')) {
      const key = node.dataset.rowKey
      const height = node.offsetHeight
      if (!key || height <= 0 || next.get(key) === height) continue
      next.set(key, height)
      changed = true
    }
    if (changed) measured.value = next
  }

  function offsetOf(index: number): number {
    return offsets.value[index] ?? 0
  }

  function indexOfKey(key: string): number {
    return rows.value.findIndex(row => row.key === key)
  }

  /**
   * Puts a row in the window and, where the list scrolls, on screen.
   *
   * The scroll position is written to the element and to the internal figure: a programmatic
   * `scrollTop` fires no scroll event in jsdom, and a jump that only the browser knows about
   * would leave the window one frame behind.
   */
  function scrollToIndex(index: number): void {
    if (index < 0) return
    const element = viewport.value
    const target = offsetOf(index)
    if (windowed.value) scrollTop.value = target
    if (!element) return
    element.scrollTop = target
  }

  function scrollToKey(key: string): boolean {
    const index = indexOfKey(key)
    if (index < 0) return false
    scrollToIndex(index)
    return true
  }

  function onScroll(): void {
    readViewport()
  }

  onMounted(() => {
    readViewport()
    window.addEventListener('resize', readViewport)
  })
  onUnmounted(() => window.removeEventListener('resize', readViewport))
  onUpdated(measure)

  // Rows that disappeared take their measurement with them, otherwise the map grows with every
  // package the user ever scrolled past.
  watch(rows, (list) => {
    if (!measured.value.size) return
    const alive = new Set(list.map(row => row.key))
    if (measured.value.size <= alive.size) return
    const next = new Map<string, number>()
    for (const [key, height] of measured.value) if (alive.has(key)) next.set(key, height)
    measured.value = next
  })

  return { viewport, windowed, rendered, padTop, padBottom, onScroll, indexOfKey, scrollToKey }
}
