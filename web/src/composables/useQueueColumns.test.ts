/**
 * The queue grid's adjustable column widths (RD-191-11): what they start at, how far they go,
 * that each list keeps its own, and that a browser without storage still draws the list.
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  QUEUE_COLUMN_DEFAULTS,
  QUEUE_COLUMN_LIMITS,
  QUEUE_COLUMNS,
  clampColumnWidth,
  queueColumnsStorageKey,
  useQueueColumns
} from './useQueueColumns'

beforeEach(() => localStorage.clear())
afterEach(() => vi.restoreAllMocks())

describe('useQueueColumns', () => {
  it('starts at the measured widths and says nothing was changed', () => {
    const columns = useQueueColumns('downloads')
    expect(columns.widths.value).toEqual({ state: 128, progress: 96, size: 144, meta: 176 })
    expect(columns.customized.value).toBe(false)
    expect(columns.style.value).toEqual({
      '--queue-col-state': '128px',
      '--queue-col-progress': '96px',
      '--queue-col-size': '144px',
      '--queue-col-meta': '176px'
    })
  })

  it('clamps a width to its column limits and rounds it', () => {
    const columns = useQueueColumns('downloads')
    expect(columns.setWidth('meta', 9999)).toBe(QUEUE_COLUMN_LIMITS.meta.max)
    expect(columns.setWidth('progress', 3)).toBe(QUEUE_COLUMN_LIMITS.progress.min)
    // The size cell's longest automatic figure is 137 px (RD-120-53); it never goes below that.
    expect(columns.setWidth('size', 100)).toBeGreaterThanOrEqual(137)
    expect(columns.setWidth('state', 200.6)).toBe(201)
    expect(clampColumnWidth('state', Number.NaN)).toBe(QUEUE_COLUMN_DEFAULTS.state)
  })

  it('keeps every default inside its own limits', () => {
    for (const column of QUEUE_COLUMNS) {
      const { min, max } = QUEUE_COLUMN_LIMITS[column]
      expect(QUEUE_COLUMN_DEFAULTS[column]).toBeGreaterThanOrEqual(min)
      expect(QUEUE_COLUMN_DEFAULTS[column]).toBeLessThanOrEqual(max)
    }
  })

  it('persists only what differs from the default, per view', () => {
    const downloads = useQueueColumns('downloads')
    downloads.setWidth('meta', 240)
    expect(JSON.parse(localStorage.getItem(queueColumnsStorageKey('downloads')) ?? 'null')).toEqual({ meta: 240 })
    expect(localStorage.getItem(queueColumnsStorageKey('linkgrabber'))).toBeNull()

    const again = useQueueColumns('downloads')
    expect(again.widths.value.meta).toBe(240)
    expect(again.customized.value).toBe(true)
    expect(again.style.value['--queue-col-meta']).toBe('240px')
    expect(useQueueColumns('linkgrabber').widths.value.meta).toBe(176)
  })

  it('resets one column, then all of them, and forgets the stored entry', () => {
    const columns = useQueueColumns('linkgrabber')
    columns.setWidth('state', 160)
    columns.setWidth('size', 200)
    columns.reset('state')
    expect(columns.widths.value.state).toBe(128)
    expect(JSON.parse(localStorage.getItem(queueColumnsStorageKey('linkgrabber')) ?? 'null')).toEqual({ size: 200 })
    columns.resetAll()
    expect(columns.widths.value).toEqual(QUEUE_COLUMN_DEFAULTS)
    expect(columns.customized.value).toBe(false)
    expect(localStorage.getItem(queueColumnsStorageKey('linkgrabber'))).toBeNull()
  })

  // The LinkGrabber leaves the progress cell empty (RD-1101-08): nothing to size, nothing kept.
  it('keeps the LinkGrabber to the columns it shows', () => {
    localStorage.setItem(queueColumnsStorageKey('linkgrabber'), JSON.stringify({ progress: 300, meta: 240 }))
    const columns = useQueueColumns('linkgrabber')
    expect(columns.widths.value.progress).toBe(QUEUE_COLUMN_DEFAULTS.progress)
    expect(columns.style.value).toEqual({ '--queue-col-state': '128px', '--queue-col-size': '144px', '--queue-col-meta': '240px' })
    columns.setWidth('meta', 200)
    expect(JSON.parse(localStorage.getItem(queueColumnsStorageKey('linkgrabber')) ?? 'null')).toEqual({ meta: 200 })
  })

  it('reads a damaged or hand-edited entry back as defaults, clamped where it can', () => {
    localStorage.setItem(queueColumnsStorageKey('downloads'), '{not json')
    expect(useQueueColumns('downloads').widths.value).toEqual(QUEUE_COLUMN_DEFAULTS)
    localStorage.setItem(queueColumnsStorageKey('downloads'), JSON.stringify({ state: 5000, name: 300, size: 'wide' }))
    expect(useQueueColumns('downloads').widths.value).toEqual({ ...QUEUE_COLUMN_DEFAULTS, state: QUEUE_COLUMN_LIMITS.state.max })
  })

  it('works without storage: defaults on read, adjustable in memory on write', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('SecurityError') })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('QuotaExceededError') })
    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => { throw new Error('SecurityError') })
    const columns = useQueueColumns('downloads')
    expect(columns.widths.value).toEqual(QUEUE_COLUMN_DEFAULTS)
    expect(() => columns.setWidth('meta', 300)).not.toThrow()
    expect(columns.style.value['--queue-col-meta']).toBe('300px')
    expect(() => columns.resetAll()).not.toThrow()
    expect(columns.widths.value.meta).toBe(176)
  })
})

describe('the queue grid in main.css', () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../assets/main.css'), 'utf8')

  // The stylesheet's fallbacks are what every row outside the two lists is drawn at, and what a
  // reset returns to; the two copies must not drift.
  it('falls back to the same defaults the composable resets to', () => {
    for (const column of QUEUE_COLUMNS) {
      expect(css).toContain(`var(--queue-col-${column}, ${QUEUE_COLUMN_DEFAULTS[column]}px)`)
      expect(css).toContain(`min(var(--queue-width-${column}), ${QUEUE_COLUMN_DEFAULTS[column]}px)`)
    }
  })

  it('sizes every data track from its property at every tier', () => {
    const tracks = [...css.matchAll(/minmax\(var\(--queue-floor-(\w+)\), var\(--queue-width-(\w+)\)\)/g)]
    expect(tracks.map(match => match[1])).toEqual(['state', 'state', 'progress', 'state', 'progress', 'size', 'meta'])
    expect(tracks.every(match => match[1] === match[2])).toBe(true)
  })
})
