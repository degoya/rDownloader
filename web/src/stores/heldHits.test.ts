import { describe, expect, it } from 'vitest'

import type { SubscriptionItem } from '@/api/types'

import { mergeUnderHand } from './heldHits'

function hit(id: string, title = id): SubscriptionItem {
  return { id, title, state: 'pending' } as SubscriptionItem
}

const ids = (items: SubscriptionItem[]) => items.map(item => item.id)

describe('mergeUnderHand (RD-1250-13)', () => {
  it('keeps the rows in their place and holds a hit written in front of them', () => {
    const merged = mergeUnderHand([hit('a'), hit('b')], [hit('new'), hit('a', 'A renamed'), hit('b')], {
      complete: true,
      decided: new Set()
    })
    expect(ids(merged.items)).toEqual(['a', 'b'])
    expect(merged.items[0]?.title).toBe('A renamed')
    expect(merged.held).toEqual(['new'])
  })

  it('lets a row go that a read from the first hit to the last no longer has', () => {
    const merged = mergeUnderHand([hit('a'), hit('b')], [hit('b')], { complete: true, decided: new Set() })
    expect(ids(merged.items)).toEqual(['b'])
    expect(merged.held).toEqual([])
  })

  it('keeps a row a full read only pushed past its window, but not one decided here', () => {
    const merged = mergeUnderHand([hit('a'), hit('b'), hit('c')], [hit('new'), hit('b')], {
      complete: false,
      decided: new Set(['a'])
    })
    expect(ids(merged.items)).toEqual(['b', 'c'])
    expect(merged.held).toEqual(['new'])
  })
})
