/**
 * The download list's sort for the eye (RD-1190-16): the click cycle, the order it gives packages
 * and the files inside them, and that it is remembered per browser without touching the queue.
 */
import { beforeEach, describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import type { Download, DownloadPackage, DownloadState } from '@/api/types'
import type { QueueGroup } from '@/composables/useQueueSelection'

import { QUEUE_SORT_STORAGE_KEY, useQueueSort } from './useQueueSort'

function file(id: string, name: string, state: DownloadState, total: number, committed = 0): Download {
  return { id, file_name: name, state, total_bytes: String(total), committed_bytes: String(committed) } as unknown as Download
}

function group(id: string, name: string, categoryId: string | null, downloads: Download[]): QueueGroup {
  return { package: { id, name, category_id: categoryId } as unknown as DownloadPackage, downloads }
}

const CATEGORIES: Record<string, string> = { films: 'Films', apps: 'Apps' }

function queue(): QueueGroup[] {
  return [
    group('p1', 'Package 10', 'films', [
      file('a', 'b-part.bin', 'completed', 300, 300),
      file('b', 'a-part.bin', 'failed', 100, 10)
    ]),
    group('p2', 'Package 9', 'apps', [file('c', 'only.bin', 'downloading', 50, 40)]),
    group('p3', 'Package 2', null, [file('d', 'paused.bin', 'paused', 1000, 0)])
  ]
}

function sorter() {
  return useQueueSort(id => (id ? CATEGORIES[id] ?? '' : ''))
}

const ids = (groups: QueueGroup[]): string[] => groups.map(entry => entry.package.id)

beforeEach(() => localStorage.clear())

describe('useQueueSort', () => {
  it('shows the queue order until a column is clicked', () => {
    const { arrange, active } = sorter()
    const groups = queue()
    expect(arrange(groups)).toBe(groups)
    expect(active.value).toBe(false)
  })

  it('goes ascending, descending and back to the queue order on the same column', () => {
    const { sort, toggle } = sorter()
    toggle('size')
    expect(sort.value).toEqual({ column: 'size', direction: 'asc' })
    toggle('size')
    expect(sort.value).toEqual({ column: 'size', direction: 'desc' })
    toggle('size')
    expect(sort.value).toBeNull()
    toggle('name')
    toggle('state')
    expect(sort.value).toEqual({ column: 'state', direction: 'asc' })
  })

  it('sorts packages by name as people count, and the files inside them too', () => {
    const { arrange, toggle } = sorter()
    toggle('name')
    const sorted = arrange(queue())
    expect(ids(sorted)).toEqual(['p3', 'p2', 'p1'])
    expect(sorted[2]!.downloads.map(item => item.id)).toEqual(['b', 'a'])
    toggle('name')
    expect(ids(arrange(queue()))).toEqual(['p1', 'p2', 'p3'])
  })

  it('sorts by size, progress, state and category on the package level', () => {
    const { arrange, toggle, reset } = sorter()
    toggle('size')
    expect(ids(arrange(queue()))).toEqual(['p2', 'p1', 'p3'])
    reset()
    toggle('progress')
    expect(ids(arrange(queue()))).toEqual(['p3', 'p1', 'p2'])
    reset()
    toggle('state')
    expect(ids(arrange(queue())), 'what runs first, then what waits, then what stopped').toEqual(['p2', 'p3', 'p1'])
    reset()
    toggle('meta')
    expect(ids(arrange(queue())), 'no category sorts first').toEqual(['p3', 'p2', 'p1'])
  })

  it('leaves the queue untouched: the arranged groups are copies', () => {
    const { arrange, toggle } = sorter()
    const groups = queue()
    toggle('name')
    arrange(groups)
    expect(ids(groups)).toEqual(['p1', 'p2', 'p3'])
    expect(groups[0]!.downloads.map(item => item.id)).toEqual(['a', 'b'])
  })

  it('is remembered per browser and ignores what it cannot read', async () => {
    const first = sorter()
    first.toggle('size')
    first.toggle('size')
    await nextTick()
    expect(JSON.parse(localStorage.getItem(QUEUE_SORT_STORAGE_KEY) ?? 'null')).toEqual({ column: 'size', direction: 'desc' })
    expect(sorter().sort.value).toEqual({ column: 'size', direction: 'desc' })
    first.reset()
    await nextTick()
    expect(localStorage.getItem(QUEUE_SORT_STORAGE_KEY), 'the queue order is the default and stores nothing').toBeNull()
    localStorage.setItem(QUEUE_SORT_STORAGE_KEY, '{"column":"nonsense","direction":"asc"}')
    expect(sorter().sort.value).toBeNull()
    localStorage.setItem(QUEUE_SORT_STORAGE_KEY, 'not json')
    expect(sorter().sort.value).toBeNull()
  })
})
