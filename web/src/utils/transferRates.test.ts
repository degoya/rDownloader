import { describe, expect, it } from 'vitest'

import { appendTransferRateHistory } from './transferRates'

describe('transfer rate history', () => {
  it('keeps a rolling two-minute graph window', () => {
    const history = appendTransferRateHistory([
      { measuredAt: 0, bytesPerSecond: 100 },
      { measuredAt: 1_000, bytesPerSecond: 200 }
    ], { measuredAt: 121_000, bytesPerSecond: 300 })

    expect(history).toEqual([{ measuredAt: 121_000, bytesPerSecond: 300 }])
  })

  it('honours a window given by the caller', () => {
    const history = appendTransferRateHistory([
      { measuredAt: 0, bytesPerSecond: 100 },
      { measuredAt: 9_000, bytesPerSecond: 200 }
    ], { measuredAt: 10_000, bytesPerSecond: 300 }, 5_000)

    expect(history.map(point => point.bytesPerSecond)).toEqual([200, 300])
  })
})
