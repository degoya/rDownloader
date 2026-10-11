import { describe, expect, it } from 'vitest'
import { computed, ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'
import { packageRowKey, useQueueSelection } from './useQueueSelection'

function file(id: string, packageId: string): Download {
  return { id, package_id: packageId } as Download
}

function group(id: string, files: Download[]) {
  return { package: { id, name: id } as DownloadPackage, downloads: files }
}

describe('useQueueSelection', () => {
  it('tracks tri-state package selection and fully selected packages', () => {
    const all = ref([file('a', 'p1'), file('b', 'p1'), file('c', 'p2')])
    const groups = ref([
      group('p1', [all.value[0]!, all.value[1]!]),
      group('p2', [all.value[2]!])
    ])
    const selection = useQueueSelection(groups, all)
    expect(selection.packageState(groups.value[0]!)).toBe('none')
    selection.setFiles(['a'], true)
    expect(selection.packageState(groups.value[0]!)).toBe('some')
    expect(selection.fullySelectedPackageIds.value).toEqual([])
    selection.togglePackage(groups.value[0]!, true)
    expect(selection.packageState(groups.value[0]!)).toBe('all')
    expect(selection.fullySelectedPackageIds.value).toEqual(['p1'])
    selection.selectAll()
    expect(selection.selectedIds.value).toEqual(['a', 'b', 'c'])
    selection.clear()
    expect(selection.selectedDownloads.value).toHaveLength(0)
  })

  it('picks only the files a filter shows, and keeps package actions for whole packages', () => {
    // `p1` holds three files; the "failed" filter shows two of them (RD-1240-31: with the filter
    // set, ticking the package selected every file of it instead of the failed ones).
    const all = ref([file('a', 'p1'), file('b', 'p1'), file('done', 'p1')])
    const shown = ref([all.value[0]!, all.value[1]!])
    const groups = computed(() => [group('p1', shown.value)])
    const selection = useQueueSelection(groups, all, undefined, shown)

    selection.togglePackage(groups.value[0]!, true)
    expect(selection.selectedIds.value).toEqual(['a', 'b'])
    expect(selection.packageState(groups.value[0]!)).toBe('all')
    // The hidden file is not ticked, so the package as a whole is not: no category, priority or
    // delete-package action reaches the file the user never saw.
    expect(selection.fullySelectedPackageIds.value).toEqual([])
    expect(selection.state.value).toBe('all')

    // Without the filter the hidden file shows up unticked; ticking the package now takes it all.
    shown.value = [...all.value]
    expect(selection.packageState(groups.value[0]!)).toBe('some')
    selection.togglePackage(groups.value[0]!, true)
    expect(selection.fullySelectedPackageIds.value).toEqual(['p1'])
  })

  it('acts only on shown files when a filter hides some that were ticked before', () => {
    const all = ref([file('a', 'p1'), file('b', 'p1')])
    const shown = ref([...all.value])
    const groups = computed(() => [group('p1', shown.value)])
    const selection = useQueueSelection(groups, all, undefined, shown)

    selection.selectAll()
    shown.value = [all.value[0]!]
    expect(selection.selectedIds.value).toEqual(['a'])
    expect(selection.count.value).toBe(1)
    expect(selection.fullySelectedPackageIds.value).toEqual([])
  })
})

describe('useQueueSelection ranges', () => {
  const ids = ['a', 'b', 'c', 'd', 'e']

  function setup() {
    const all = ref(ids.map(id => file(id, 'p1')))
    const groups = ref([group('p1', all.value)])
    const ordered = computed(() => ids)
    return useQueueSelection(groups, all, ordered)
  }

  /**
   * Range selection did not exist before RD-106-12 — `shiftKey` appeared nowhere in `web/src`.
   * It follows the order the rows are on screen in, which with a virtualized list is the
   * flattened row stream and not the store's order.
   */
  it('selects from the anchor to the shift-picked row', () => {
    const selection = setup()
    selection.pickFile('b', true)
    selection.pickFile('d', true, true)
    expect(selection.selectedIds.value).toEqual(['b', 'c', 'd'])
  })

  it('extends backwards just as well', () => {
    const selection = setup()
    selection.pickFile('d', true)
    selection.pickFile('b', true, true)
    expect(selection.selectedIds.value).toEqual(['b', 'c', 'd'])
  })

  /** The anchor stays put, so a second shift-click corrects the first instead of starting over. */
  it('keeps the anchor while the range is stretched', () => {
    const selection = setup()
    selection.pickFile('b', true)
    selection.pickFile('e', true, true)
    selection.pickFile('c', false, true)
    expect(selection.selectedIds.value).toEqual(['d', 'e'])
    expect(selection.anchor.value).toBe('b')
  })

  /** Shift without a previous plain pick has nothing to reach from; it picks the one row. */
  it('falls back to a single pick without an anchor', () => {
    const selection = setup()
    selection.pickFile('c', true, true)
    expect(selection.selectedIds.value).toEqual(['c'])
  })

  it('forgets the anchor when the selection is cleared', () => {
    const selection = setup()
    selection.pickFile('b', true)
    selection.clear()
    expect(selection.anchor.value).toBeNull()
  })
})

describe('useQueueSelection package ranges (RD-170-13)', () => {
  /** p1 open, p2 collapsed, p3 open with one of its two files hidden by a filter. */
  function setup() {
    const all = ref([file('a', 'p1'), file('b', 'p1'), file('c', 'p2'), file('d', 'p3'), file('hidden', 'p3')])
    const groups = ref([group('p1', [all.value[0]!, all.value[1]!]), group('p2', [all.value[2]!]), group('p3', [all.value[3]!])])
    const ordered = computed(() => [packageRowKey('p1'), 'a', 'b', packageRowKey('p2'), packageRowKey('p3'), 'd'])
    return useQueueSelection(groups, all, ordered)
  }

  it('selects every package from the anchor to the shift-clicked one', () => {
    const selection = setup()
    selection.pickPackage('p1', true)
    selection.pickPackage('p3', true, true)
    expect(selection.selectedIds.value).toEqual(['a', 'b', 'c', 'd', 'hidden'])
  })

  it('takes a collapsed package inside a file range whole', () => {
    const selection = setup()
    selection.pickFile('b', true)
    selection.pickFile('d', true, true)
    expect(selection.selectedIds.value).toEqual(['b', 'c', 'd'])
  })
})

describe('useQueueSelection size', () => {
  function sized(id: string, packageId: string, totalBytes: string | null): Download {
    return { ...file(id, packageId), total_bytes: totalBytes }
  }

  it('counts a ticked package and its ticked files once (RD-170-14)', () => {
    const all = ref([sized('a', 'p1', '1000'), sized('b', 'p1', null), sized('c', 'p2', '3000')])
    const groups = ref([group('p1', [all.value[0]!, all.value[1]!]), group('p2', [all.value[2]!])])
    const selection = useQueueSelection(groups, all)

    selection.pickFile('a', true)
    selection.pickPackage('p1', true)

    expect(selection.size.value).toEqual({ count: 2, bytes: 1000n, unknown: 1 })
    selection.selectAll()
    expect(selection.size.value).toEqual({ count: 3, bytes: 4000n, unknown: 1 })
  })
})
