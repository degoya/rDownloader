import { CalendarDate, Time } from '@internationalized/date'
import { describe, expect, it } from 'vitest'

import { clockOf, dateFieldValue, dayOf, timeFieldValue } from './timeFields'
import { minutesOf, timeOf } from './weekWindows'

describe('time field values', () => {
  it('hands a stored clock time back unchanged when the field is not touched', () => {
    for (const clock of ['00:00', '07:05', '20:00', '23:59']) {
      expect(clockOf(timeFieldValue(clock))).toBe(clock)
    }
  })

  it('keeps every minute of a week window as it was stored', () => {
    for (const minutes of [0, 1, 7 * 60, 23 * 60 + 59]) {
      expect(minutesOf(clockOf(timeFieldValue(timeOf(minutes))))).toBe(minutes)
    }
  })

  it('shows the end of a day, 24:00, as midnight', () => {
    expect(timeFieldValue('24:00')?.compare(new Time(0, 0))).toBe(0)
  })

  it('leaves the field empty for an empty or unreadable value, and empties to the empty string', () => {
    expect(timeFieldValue('')).toBeUndefined()
    expect(timeFieldValue('25:00')).toBeUndefined()
    expect(timeFieldValue('12:75')).toBeUndefined()
    expect(clockOf(undefined)).toBe('')
    expect(clockOf(null)).toBe('')
  })

  it('reads the hour and minute of a value with seconds',() => {
    expect(clockOf(new Time(8, 15, 30))).toBe('08:15')
  })
})

describe('date field values', () => {
  it('hands a stored day back unchanged when the field is not touched', () => {
    for (const day of ['2027-01-01', '2026-02-28', '2026-12-31']) {
      expect(dayOf(dateFieldValue(day))).toBe(day)
    }
  })

  it('leaves the field empty for no day, and empties to the empty string', () => {
    expect(dateFieldValue('')).toBeUndefined()
    expect(dateFieldValue(null)).toBeUndefined()
    expect(dateFieldValue('2026-13-01')).toBeUndefined()
    expect(dateFieldValue('01.02.2026')).toBeUndefined()
    expect(dayOf(undefined)).toBe('')
  })

  it('writes the day the field picked as YYYY-MM-DD', () => {
    expect(dayOf(new CalendarDate(2026, 3, 5))).toBe('2026-03-05')
  })
})
