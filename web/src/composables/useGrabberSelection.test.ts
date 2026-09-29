import { describe, expect, it } from 'vitest'
import { computed, ref } from 'vue'

import type { CollectorPackage, LinkCandidate, NzbImport } from '@/api/types'

import { grabberKey, isSelectableCandidate, mergeGrabberEntries, packageRowKey, useGrabberSelection, type CollectorEntry, type GrabberEntry, type NzbEntry } from './useGrabberSelection'

function candidate(id: string): LinkCandidate {
  return { id, state: 'online', url: `https://files.example.com/${id}` } as LinkCandidate
}

function collectorEntry(id: string, ids: string[], position = 0, createdAt = '2026-09-01T10:00:00Z'): CollectorEntry {
  return {
    kind: 'collector',
    id,
    createdAt,
    position,
    package: { id, name: id } as CollectorPackage,
    candidates: ids.map(candidate)
  }
}

function nzbEntry(id: string, position = 0, createdAt = '2026-09-01T11:00:00Z'): NzbEntry {
  return { kind: 'nzb', id, createdAt, position, item: { id, name: id } as NzbImport }
}

describe('mergeGrabberEntries', () => {
  /**
   * Both kinds sit in one manual order now, and creation time contradicts it on purpose here:
   * the import was added last and belongs first. That is the whole point of giving its row a
   * handle — before this, a timestamp decided where an NZB row went and nothing could move it.
   */
  it('orders both kinds by the position they share, not by creation time', () => {
    const merged = mergeGrabberEntries(
      [collectorEntry('p1', ['a'], 2, '2026-09-01T09:00:00Z'), collectorEntry('p2', ['b'], 3, '2026-09-01T10:00:00Z')],
      [nzbEntry('n1', 1, '2026-09-01T12:00:00Z')]
    )
    expect(merged.map(entry => entry.id)).toEqual(['n1', 'p1', 'p2'])
  })

  /** Two sources, one sort: a listing not yet renumbered must still come out in a fixed order. */
  it('falls back to creation time where two entries share a position', () => {
    const merged = mergeGrabberEntries(
      [collectorEntry('p1', ['a'], 0, '2026-09-01T12:00:00Z')],
      [nzbEntry('n1', 0, '2026-09-01T09:00:00Z')]
    )
    expect(merged.map(entry => entry.id)).toEqual(['n1', 'p1'])
  })
})

describe('useGrabberSelection', () => {
  const keys = ['a', 'b', 'c', 'd', 'e']

  function setup() {
    const entries = ref<GrabberEntry[]>([collectorEntry('p1', keys), nzbEntry('n1')])
    const ordered = computed(() => [
      ...keys.map(id => grabberKey('collector', id)),
      grabberKey('nzb', 'n1')
    ])
    return useGrabberSelection(entries, ordered)
  }

  it('drops stale keys instead of reporting a selection that is gone', () => {
    const entries = ref<GrabberEntry[]>([collectorEntry('p1', ['a', 'b'])])
    const selection = useGrabberSelection(entries)
    selection.setCollector(['a', 'b'], true)
    expect(selection.collectorIds.value).toEqual(['a', 'b'])
    entries.value = [collectorEntry('p1', ['a'])]
    expect(selection.collectorIds.value).toEqual(['a'])
  })

  /**
   * Range selection did not exist before RD-106-12 — `shiftKey` appeared nowhere in `web/src`.
   * It follows the order the rows are on screen in, which with a virtualized list is the
   * flattened row stream and not the store's order.
   */
  it('selects from the anchor to the shift-picked row', () => {
    const selection = setup()
    selection.pickCollector('b', true)
    selection.pickCollector('d', true, true)
    expect(selection.collectorIds.value).toEqual(['b', 'c', 'd'])
  })

  it('extends backwards just as well', () => {
    const selection = setup()
    selection.pickCollector('d', true)
    selection.pickCollector('b', true, true)
    expect(selection.collectorIds.value).toEqual(['b', 'c', 'd'])
  })

  /** The list mixes both kinds, and a range across the boundary takes what it crosses. */
  it('reaches across the boundary between links and NZB imports', () => {
    const selection = setup()
    selection.pickCollector('d', true)
    selection.pickNzb('n1', true, true)
    expect(selection.collectorIds.value).toEqual(['d', 'e'])
    expect(selection.nzbIds.value).toEqual(['n1'])
  })

  /** Shift without a previous plain pick has nothing to reach from; it picks the one row. */
  it('falls back to a single pick without an anchor', () => {
    const selection = setup()
    selection.pickCollector('c', true, true)
    expect(selection.collectorIds.value).toEqual(['c'])
  })

  it('forgets the anchor when the selection is cleared', () => {
    const selection = setup()
    selection.pickCollector('b', true)
    selection.clear()
    expect(selection.anchor.value).toBeNull()
  })
})

describe('useGrabberSelection package ranges (RD-170-13)', () => {
  /** p1 open with its links on screen, p2 collapsed, then the import and an open p3. */
  function setup() {
    const entries = ref<GrabberEntry[]>([
      collectorEntry('p1', ['a', 'b'], 0),
      collectorEntry('p2', ['c', 'd'], 1),
      nzbEntry('n1', 2),
      collectorEntry('p3', ['e'], 3)
    ])
    const ordered = computed(() => [
      packageRowKey('p1'), grabberKey('collector', 'a'), grabberKey('collector', 'b'),
      packageRowKey('p2'),
      grabberKey('nzb', 'n1'),
      packageRowKey('p3'), grabberKey('collector', 'e')
    ])
    return useGrabberSelection(entries, ordered)
  }

  it('selects every package from the anchor to the shift-clicked one', () => {
    const selection = setup()
    selection.pickPackage('p1', true)
    selection.pickPackage('p3', true, true)
    expect(selection.collectorIds.value).toEqual(['a', 'b', 'c', 'd', 'e'])
    expect(selection.nzbIds.value).toEqual(['n1'])
  })

  it('takes a collapsed package between a link and an import whole', () => {
    const selection = setup()
    selection.pickNzb('n1', true)
    selection.pickCollector('b', true, true)
    expect(selection.collectorIds.value).toEqual(['b', 'c', 'd'])
  })

  /** An open package in the middle adds only the rows it shows, which are in the range anyway. */
  it('does not pull in the rest of an open package the range only crosses', () => {
    const selection = setup()
    selection.pickCollector('b', true)
    selection.pickPackage('p2', true, true)
    expect(selection.collectorIds.value).toEqual(['b', 'c', 'd'])
  })
})

describe('isSelectableCandidate', () => {
  // A row shows its checkbox for every state a link may be queued from; a checkbox that
  // cannot be ticked was the bug for `unsupported` and `error`.
  it('follows the enqueueable states', () => {
    for (const state of ['online', 'duplicate', 'offline', 'unsupported', 'error']) {
      expect(isSelectableCandidate({ state } as LinkCandidate), state).toBe(true)
    }
    for (const state of ['unresolvable', 'checking', 'queued']) {
      expect(isSelectableCandidate({ state } as LinkCandidate), state).toBe(false)
    }
  })
})

describe('useGrabberSelection size', () => {
  function sized(entry: CollectorEntry, sizes: Record<string, string | null>): CollectorEntry {
    return { ...entry, candidates: entry.candidates.map(link => ({ ...link, size: sizes[link.id] ?? null })) }
  }

  it('counts a ticked package and its ticked links once (RD-170-14)', () => {
    const nzb = nzbEntry('n1', 2)
    const entries = ref<GrabberEntry[]>([
      sized(collectorEntry('p1', ['a', 'b'], 1), { a: '1000', b: '2000' }),
      { ...nzb, item: { ...nzb.item, total_bytes: '500' } }
    ])
    const selection = useGrabberSelection(entries)

    selection.pickCollector('a', true)
    selection.pickPackage('p1', true)
    selection.pickNzb('n1', true)

    expect(selection.size.value).toEqual({ count: 3, bytes: 3500n, unknown: 0 })
  })

  it('keeps a link without a size out of the sum and counts it apart', () => {
    const entries = ref<GrabberEntry[]>([sized(collectorEntry('p1', ['a', 'b']), { a: '1000', b: null })])
    const selection = useGrabberSelection(entries)

    selection.pickPackage('p1', true)

    expect(selection.size.value).toEqual({ count: 2, bytes: 1000n, unknown: 1 })
    selection.clear()
    expect(selection.size.value.count).toBe(0)
  })
})
