import { describe, expect, it, vi } from 'vitest'

import { BULK_LIMIT, batchError, batches, combinedMessage, inBatches } from './bulkBatches'

const ids = (count: number) => Array.from({ length: count }, (_, index) => `id-${index}`)

describe('bulk batches', () => {
  it('splits a selection into slices the server accepts', () => {
    expect(batches(ids(733)).map(slice => slice.length)).toEqual([500, 233])
    expect(batches(ids(500)).map(slice => slice.length)).toEqual([500])
    expect(batches([])).toEqual([])
    expect(BULK_LIMIT).toBe(500)
  })

  it('sends the batches one after the other and stops at the first refusal', async () => {
    const send = vi.fn()
      .mockResolvedValueOnce({ data: 'first' })
      .mockResolvedValueOnce({ error: { error: 'refused', code: 'unknown.code' } })
    const run = await inBatches(ids(1200), send)

    expect(send).toHaveBeenCalledTimes(2)
    expect(run.data).toEqual(['first'])
    expect(run.sent.map(slice => slice.length)).toEqual([500])
    expect(run.total).toBe(3)
    expect(batchError(run)).toBe('refused – stopped after part 1 of 3; the parts before it were applied.')
  })

  it('reports a refused first batch as the plain error', async () => {
    const run = await inBatches(ids(2), async () => ({ data: undefined, error: { error: 'refused', code: 'unknown.code' } }))

    expect(batchError(run)).toBe('refused')
  })

  it('adds up the counts of the batches into one message', () => {
    expect(combinedMessage([
      { code: 'package.bulk_removed', message: '500 package(s) removed', params: { count: 500 } },
      { code: 'package.bulk_removed', message: '233 package(s) removed', params: { count: 233 } }
    ])).toBe('733 packages removed from the download list')
  })
})
