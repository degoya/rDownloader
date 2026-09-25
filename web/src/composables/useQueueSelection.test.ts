import { describe, expect, it } from 'vitest'
import { computed, ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'
import { useQueueSelection } from './useQueueSelection'

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

  it('judges a package by every file it has, not by the ones a filter leaves visible', () => {
    // `p1` holds two files; the active filter shows only the first — the shape an NZB package
    // has while its `.par2` files sit in another state than its payload.
    const all = ref([file('a', 'p1'), file('hidden', 'p1')])
    const groups = computed(() => [group('p1', [all.value[0]!])])
    const selection = useQueueSelection(groups, all)

    selection.setFiles(['a'], true)
    expect(selection.packageState(groups.value[0]!)).toBe('some')
    expect(selection.fullySelectedPackageIds.value).toEqual([])

    // Ticking the package takes the hidden file with it, so the package-level actions apply to
    // the package the user pointed at rather than to a part of it.
    selection.togglePackage(groups.value[0]!, true)
    expect(selection.selectedIds.value).toEqual(['a', 'hidden'])
    expect(selection.fullySelectedPackageIds.value).toEqual(['p1'])
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
