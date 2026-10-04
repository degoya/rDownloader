import { computed, ref } from 'vue'

/**
 * Adjustable widths of the queue grid's data columns (RD-191-11).
 *
 * The download list and the LinkGrabber share `.queue-row` (`web/src/assets/main.css`), whose
 * four data columns — state, progress, size, metadata — read their width from a custom property
 * each, `--queue-col-<column>`, with the measured widths of RD-109-30 / RD-120-53 as the
 * fallback. This keeps what the viewer dragged them to and hands it to the list container as
 * those properties, so every row under it — and the column header above it — follows at once.
 *
 * A view preference of one viewer in one browser, so it lives in `localStorage` under one key
 * per view, the way `rdownloader-open-packages` does. Every access is guarded: without storage
 * (private mode, blocked site data) the widths are simply the defaults and still adjustable for
 * as long as the view lives. Stored are only the columns that differ from their default.
 */
export type QueueColumn = 'state' | 'progress' | 'size' | 'meta'
export type QueueColumnsView = 'downloads' | 'linkgrabber'

export const QUEUE_COLUMNS: readonly QueueColumn[] = ['state', 'progress', 'size', 'meta']

/** The widths `.queue-row` falls back to; changing one here means changing it in `main.css`. */
export const QUEUE_COLUMN_DEFAULTS: Readonly<Record<QueueColumn, number>> = { state: 128, progress: 96, size: 144, meta: 176 }

/**
 * How far a column may go. The floors are what the cell still says something at: the size's
 * longest automatic figure, `1023 MiB / 99.9 GiB`, is 137 px of 12 px mono (RD-120-53); a state
 * badge and the package's `3/12 · 2 active` read at 96 px; the metadata cell keeps a shrunk
 * category select and the priority glyph. The ceiling stops one column from eating the row —
 * the grid itself still gives the name its 200 px first (`main.css`).
 */
export const QUEUE_COLUMN_LIMITS: Readonly<Record<QueueColumn, { min: number, max: number }>> = {
  state: { min: 96, max: 480 },
  progress: { min: 64, max: 480 },
  size: { min: 140, max: 480 },
  meta: { min: 96, max: 480 }
}

export function queueColumnsStorageKey(view: QueueColumnsView): string {
  return `rdownloader-queue-columns-${view}`
}

export function clampColumnWidth(column: QueueColumn, width: number): number {
  const { min, max } = QUEUE_COLUMN_LIMITS[column]
  if (!Number.isFinite(width)) return QUEUE_COLUMN_DEFAULTS[column]
  return Math.min(max, Math.max(min, Math.round(width)))
}

function isColumn(name: string): name is QueueColumn {
  return (QUEUE_COLUMNS as readonly string[]).includes(name)
}

export function useQueueColumns(view: QueueColumnsView) {
  const storageKey = queueColumnsStorageKey(view)

  function read(): Record<QueueColumn, number> {
    const widths = { ...QUEUE_COLUMN_DEFAULTS }
    try {
      const raw = localStorage.getItem(storageKey)
      const stored: unknown = raw ? JSON.parse(raw) : null
      if (stored && typeof stored === 'object') {
        for (const [name, value] of Object.entries(stored)) {
          if (isColumn(name) && typeof value === 'number') widths[name] = clampColumnWidth(name, value)
        }
      }
    } catch {
      // Unreadable or unavailable storage: the defaults stand.
    }
    return widths
  }

  const widths = ref<Record<QueueColumn, number>>(read())

  function persist(): void {
    const changed = Object.fromEntries(QUEUE_COLUMNS
      .filter(column => widths.value[column] !== QUEUE_COLUMN_DEFAULTS[column])
      .map(column => [column, widths.value[column]]))
    try {
      if (Object.keys(changed).length) localStorage.setItem(storageKey, JSON.stringify(changed))
      else localStorage.removeItem(storageKey)
    } catch {
      // Storage unavailable (private mode, quota) — the in-memory widths still apply.
    }
  }

  /** Sets a column's width, clamped to its limits; returns what it was set to. */
  function setWidth(column: QueueColumn, width: number): number {
    const next = clampColumnWidth(column, width)
    if (next !== widths.value[column]) {
      widths.value = { ...widths.value, [column]: next }
      persist()
    }
    return next
  }

  function reset(column: QueueColumn): void {
    setWidth(column, QUEUE_COLUMN_DEFAULTS[column])
  }

  function resetAll(): void {
    widths.value = { ...QUEUE_COLUMN_DEFAULTS }
    persist()
  }

  /** Whether any column differs from its default — what enables "reset all". */
  const customized = computed(() => QUEUE_COLUMNS.some(column => widths.value[column] !== QUEUE_COLUMN_DEFAULTS[column]))

  /** The custom properties for the container that holds the header row and the rows. */
  const style = computed<Record<string, string>>(() => Object.fromEntries(
    QUEUE_COLUMNS.map(column => [`--queue-col-${column}`, `${widths.value[column]}px`])
  ))

  return { widths, setWidth, reset, resetAll, customized, style }
}
