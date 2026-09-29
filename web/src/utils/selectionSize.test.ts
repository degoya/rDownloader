import { describe, expect, it } from 'vitest'

import { sumSelection } from './selectionSize'

describe('sumSelection', () => {
  it('adds the known sizes and counts the ones nobody knows yet', () => {
    expect(sumSelection(['1024', null, '2048', undefined])).toEqual({ count: 4, bytes: 3072n, unknown: 2 })
  })

  it('treats a size that is no number as unknown rather than failing', () => {
    expect(sumSelection(['12', 'n/a'])).toEqual({ count: 2, bytes: 12n, unknown: 1 })
  })

  it('is empty for an empty selection', () => {
    expect(sumSelection([])).toEqual({ count: 0, bytes: 0n, unknown: 0 })
  })
})
