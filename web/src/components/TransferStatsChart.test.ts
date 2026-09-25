/**
 * The statistics bars sit on the range's time axis (RD-120-48).
 *
 * The screenshot run drew one busy hour as a block across the whole chart, with the same moment
 * under both ends of the axis: the bars were laid out by their index in a list that only holds
 * the buckets that moved something.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import stats from '@/locales/en/stats.json'
import { mountComponent } from '@/test/mount'

import TransferStatsChart from './TransferStatsChart.vue'

const FIGURES = { completed: 1, failed: 0, retries: 0, seconds: 1 }

function mount(props: Record<string, unknown>) {
  return mountComponent(TransferStatsChart, { props, messages: { stats } })
}

function bars(): { x: number, width: number }[] {
  return [...document.querySelectorAll('rect')].map(rect => ({
    x: Number(rect.getAttribute('x')),
    width: Number(rect.getAttribute('width'))
  }))
}

function axisLabels(): string[] {
  return [...document.querySelectorAll('[aria-hidden="true"] > span')].map(span => span.textContent ?? '')
}

describe('TransferStatsChart', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-23T21:30:00Z'))
  })
  afterEach(() => vi.useRealTimers())

  it('draws a single hour as one narrow bar at its hour, not across the chart', () => {
    mount({
      resolution: 'hour',
      since: '2026-09-22T21:30:00Z',
      buckets: [{ start: '2026-09-23T21:00:00Z', bytes: 144_703_488, ...FIGURES }]
    })
    const [bar, ...rest] = bars()
    expect(rest).toEqual([])
    // 25 hourly slots from 21:00 yesterday to 21:00 today; this bucket is the last of them.
    expect(bar?.width).toBeLessThan(640 / 24)
    expect(bar?.x).toBeGreaterThan(640 * 23 / 25)
    const [first, last] = axisLabels()
    expect(first).toBeTruthy()
    expect(first).not.toBe(last)
  })

  it('keeps the gap between two busy days', () => {
    mount({
      resolution: 'day',
      since: '2026-09-16T21:30:00Z',
      buckets: [
        { start: '2026-09-17T00:00:00Z', bytes: 1_000, ...FIGURES },
        { start: '2026-09-21T00:00:00Z', bytes: 2_000, ...FIGURES }
      ]
    })
    const [early, late] = bars()
    // Eight daily slots (16th to 23rd): the two bars are four slots apart, not neighbours.
    expect((late?.x ?? 0) - (early?.x ?? 0)).toBeCloseTo(640 / 8 * 4)
  })
})
