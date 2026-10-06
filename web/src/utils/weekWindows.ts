/**
 * The weekly time window the quiet hours, the reconnect windows and the bandwidth schedule share
 * (RD-1120-15): a day bitmask, Monday in bit 0 as the backend stores it, and a start and an end
 * in minutes after midnight. An end at or before the start runs past midnight.
 */
export interface WeekWindow {
  days: number
  start_minute: number
  end_minute: number
}

type Translate = (key: string) => string

/** Monday first, the order of the backend's bitmask and of `common.weekdays`. */
const WEEKDAYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] as const

/** Every day of the week as a bitmask. */
export const EVERY_DAY = 0b0111_1111

/**
 * The days as items of a checkbox group, labelled from `common.weekdays`. `first` is the value
 * Monday carries: 0 for a bit position, 1 for an ISO weekday (stream schedules).
 */
export function weekdayItems(t: Translate, first = 0): { value: number, label: string }[] {
  return WEEKDAYS.map((day, index) => ({ value: index + first, label: t(`common.weekdays.${day}`) }))
}

/** The bitmask as the list of day positions a checkbox group holds. */
export function daysOf(mask: number): number[] {
  return WEEKDAYS.map((_, day) => day).filter(day => (mask & (1 << day)) !== 0)
}

/** The ticked day positions back as a bitmask. */
export function maskOf(days: number[]): number {
  return days.reduce((mask, day) => mask | (1 << day), 0)
}

/** Minutes after midnight as the `HH:MM` a time field holds. */
export function timeOf(minutes: number): string {
  const hours = Math.floor(minutes / 60)
  return `${String(hours).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`
}

/** A time field's `HH:MM` as minutes after midnight, held to 0…1440; anything else is 0. */
export function minutesOf(value: string): number {
  const [hours, minutes] = value.split(':').map(Number)
  const total = (hours ?? 0) * 60 + (minutes ?? 0)
  return Number.isFinite(total) ? Math.min(Math.max(total, 0), 1440) : 0
}
