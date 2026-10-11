import { describe, expect, it } from 'vitest'

import { describeSpan, draftOf, localPosition, sameWindow, windowBody, windowOpen } from './downloadWindow'

const BERLIN = 'Europe/Berlin'
const night = { windows: [{ days: 127, start_minute: 22 * 60, end_minute: 6 * 60 }], ignore_schedule_pause: false }

describe('download window (RD-1240-30)', () => {
  it('reads the local weekday and minute in the schedule timezone', () => {
    // 2026-01-15 is a Thursday; 22:30 UTC is 23:30 in Berlin (CET).
    expect(localPosition(BERLIN, new Date('2026-01-15T22:30:00Z'))).toEqual({ weekday: 3, minute: 23 * 60 + 30 })
    // Midnight is minute 0, never 24:00.
    expect(localPosition('UTC', new Date('2026-01-16T00:00:00Z'))).toEqual({ weekday: 4, minute: 0 })
  })

  it('opens a window that wraps midnight on both sides of it and closes it at its end', () => {
    expect(windowOpen(night, BERLIN, new Date('2026-01-15T22:30:00Z'))).toBe(true)
    expect(windowOpen(night, BERLIN, new Date('2026-01-16T04:59:00Z'))).toBe(true)
    expect(windowOpen(night, BERLIN, new Date('2026-01-16T05:00:00Z'))).toBe(false)
    expect(windowOpen(night, BERLIN, new Date('2026-01-16T11:00:00Z'))).toBe(false)
  })

  it('lets a wrapping span belong to the day it starts on', () => {
    const friday = { windows: [{ days: 1 << 4, start_minute: 22 * 60, end_minute: 2 * 60 }], ignore_schedule_pause: false }
    // Saturday 01:00 in Berlin is still Friday's span; Thursday 23:00 is not a Friday.
    expect(windowOpen(friday, BERLIN, new Date('2026-01-17T00:00:00Z'))).toBe(true)
    expect(windowOpen(friday, BERLIN, new Date('2026-01-15T22:00:00Z'))).toBe(false)
  })

  it('follows the local clock across the daylight-saving change', () => {
    const early = { windows: [{ days: 127, start_minute: 0, end_minute: 3 * 60 }], ignore_schedule_pause: false }
    // 2026-03-29: 00:30 UTC is 01:30 CET, 01:00 UTC is already 03:00 CEST.
    expect(windowOpen(early, BERLIN, new Date('2026-03-29T00:30:00Z'))).toBe(true)
    expect(windowOpen(early, BERLIN, new Date('2026-03-29T01:00:00Z'))).toBe(false)
  })

  it('keeps a window without spans open', () => {
    expect(windowOpen({ windows: [], ignore_schedule_pause: true }, BERLIN, new Date())).toBe(true)
  })

  it('turns a draft into the request body and back', () => {
    const draft = draftOf(night)
    expect(draft.enabled).toBe(true)
    expect(windowBody(draft)).toEqual(night)
    expect(windowBody({ ...draft, enabled: false })).toBeNull()
    expect(draftOf(null)).toEqual({ enabled: false, windows: [], ignore_schedule_pause: false })
    expect(sameWindow(windowBody(draft), night)).toBe(true)
    expect(sameWindow(null, undefined)).toBe(true)
    expect(sameWindow(night, null)).toBe(false)
  })

  it('names the days as a range where they run on', () => {
    const t = (key: string) => `<${key.split('.').pop()}>`
    expect(describeSpan({ days: 127, start_minute: 22 * 60, end_minute: 6 * 60 }, t)).toBe('<mon>–<sun> 22:00–06:00')
    expect(describeSpan({ days: 0b101, start_minute: 60, end_minute: 120 }, t)).toBe('<mon>, <wed> 01:00–02:00')
  })
})
