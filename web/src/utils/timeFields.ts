/**
 * The values `UInputTime` and `UInputDate` hold, to and from the strings the settings and the
 * API keep (RD-1120-23). The fields work on `@internationalized/date` objects; what is stored
 * stays a `HH:MM` clock time or a `YYYY-MM-DD` day, so a field that is not touched hands back
 * exactly what it was given.
 */
import { CalendarDate, Time } from '@internationalized/date'

/** What a time field emits: a `Time`, or nothing while it is empty. */
export type TimeLike = { hour: number, minute: number } | null | undefined
/** What a date field emits: a day, or nothing while it is empty. */
export type DateLike = { year: number, month: number, day: number } | null | undefined

const CLOCK = /^(\d{1,2}):(\d{2})/
const DAY = /^(\d{4})-(\d{2})-(\d{2})$/

const pad = (value: number, width = 2) => String(value).padStart(width, '0')

/** A `HH:MM` clock time as the time field's value; nothing for an empty or unreadable one. */
export function timeFieldValue(clock: string): Time | undefined {
  const match = CLOCK.exec(clock.trim())
  if (!match) return undefined
  const hours = Number(match[1])
  const minutes = Number(match[2])
  // `24:00`, the end of a day as a window stores it, is the field's midnight.
  if (hours > 24 || minutes > 59) return undefined
  return new Time(hours % 24, minutes)
}

/** The time field's value as `HH:MM`; an emptied field is the empty string. */
export function clockOf(value: TimeLike): string {
  return value ? `${pad(value.hour)}:${pad(value.minute)}` : ''
}

/** A `YYYY-MM-DD` day as the date field's value; nothing for an empty or unreadable one. */
export function dateFieldValue(day: string | null | undefined): CalendarDate | undefined {
  const match = DAY.exec(day?.trim() ?? '')
  if (!match) return undefined
  const [year, month, date] = match.slice(1).map(Number) as [number, number, number]
  if (month < 1 || month > 12 || date < 1 || date > 31) return undefined
  return new CalendarDate(year, month, date)
}

/** The date field's value as `YYYY-MM-DD`; an emptied field is the empty string. */
export function dayOf(value: DateLike): string {
  return value ? `${pad(value.year, 4)}-${pad(value.month)}-${pad(value.day)}` : ''
}
