import { describe, expect, it } from 'vitest'

import { sumSelection, type SelectionParts } from './selectionSize'

const parts: SelectionParts = { packages: 1, items: 2, unit: 'file', imports: 0 }

describe('sumSelection', () => {
  it('adds the known sizes and counts the ones nobody knows yet', () => {
    expect(sumSelection(['1024', null, '2048', undefined], parts)).toEqual({ count: 4, bytes: 3072n, unknown: 2, parts })
  })

  it('treats a size that is no number as unknown rather than failing', () => {
    expect(sumSelection(['12', 'n/a'], parts)).toEqual({ count: 2, bytes: 12n, unknown: 1, parts })
  })

  it('is empty for an empty selection', () => {
    expect(sumSelection([], parts)).toEqual({ count: 0, bytes: 0n, unknown: 0, parts })
  })
})
