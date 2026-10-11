/**
 * A package's or a category's download window (RD-1240-30): weekly spans in the bandwidth
 * schedule's timezone during which its files may download, and whether it downloads while the
 * schedule's profile pauses downloads. The service decides; this reads the same rule for the
 * row's glyph — a span that wraps midnight belongs to the day it starts on, as in
 * `crates/rd-limits/src/schedule.rs` — and turns the form's draft into the request body.
 */
import type { DownloadWindow } from '@/api/types'
import { EVERY_DAY, timeOf, type WeekWindow } from '@/utils/weekWindows'

/** What the editor holds: whether a window is set at all, its spans and the bypass. */
export interface DownloadWindowDraft {
  enabled: boolean
  windows: WeekWindow[]
  ignore_schedule_pause: boolean
}

/** The most spans the service takes (`MAX_DOWNLOAD_WINDOW_SPANS`). */
export const MAX_SPANS = 28

/** The span a new row starts with: every night from 22:00 to 06:00, the owner's example. */
export function nightSpan(): WeekWindow {
  return { days: EVERY_DAY, start_minute: 22 * 60, end_minute: 6 * 60 }
}

/** The draft of a stored window; none is a switched-off draft. */
export function draftOf(window: DownloadWindow | null | undefined): DownloadWindowDraft {
  return {
    enabled: Boolean(window),
    windows: (window?.windows ?? []).map(span => ({ ...span })),
    ignore_schedule_pause: window?.ignore_schedule_pause ?? false
  }
}

/** The body the routes take: the window, or `null` to remove it. */
export function windowBody(draft: DownloadWindowDraft): DownloadWindow | null {
  if (!draft.enabled) return null
  return {
    windows: draft.windows.map(span => ({ days: span.days, start_minute: span.start_minute, end_minute: span.end_minute })),
    ignore_schedule_pause: draft.ignore_schedule_pause
  }
}

/** Whether two windows say the same, so an unchanged one is not sent again. */
export function sameWindow(left: DownloadWindow | null | undefined, right: DownloadWindow | null | undefined): boolean {
  return JSON.stringify(left ?? null) === JSON.stringify(right ?? null)
}

const WEEKDAY_INDEX: Record<string, number> = { Mon: 0, Tue: 1, Wed: 2, Thu: 3, Fri: 4, Sat: 5, Sun: 6 }

/**
 * The weekday (Monday 0) and the minute after midnight of `now` in `timeZone`; the browser's own
 * zone when the name is unknown to it.
 */
export function localPosition(timeZone: string, now: Date): { weekday: number, minute: number } {
  let parts: Intl.DateTimeFormatPart[]
  try {
    parts = new Intl.DateTimeFormat('en-US', { timeZone, weekday: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(now)
  } catch {
    parts = new Intl.DateTimeFormat('en-US', { weekday: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(now)
  }
  const part = (type: string) => parts.find(entry => entry.type === type)?.value ?? '0'
  return {
    weekday: WEEKDAY_INDEX[part('weekday')] ?? 0,
    minute: (Number(part('hour')) % 24) * 60 + Number(part('minute'))
  }
}

function covers(span: WeekWindow, weekday: number, minute: number): boolean {
  const on = (day: number) => (span.days & (1 << day)) !== 0
  if (span.start_minute < span.end_minute) return on(weekday) && minute >= span.start_minute && minute < span.end_minute
  return (on(weekday) && minute >= span.start_minute) || (on((weekday + 6) % 7) && minute < span.end_minute)
}

/** Whether `window` lets its package download at `now`; a window without spans always does. */
export function windowOpen(window: DownloadWindow, timeZone: string, now: Date): boolean {
  if (!window.windows?.length) return true
  const { weekday, minute } = localPosition(timeZone, now)
  return window.windows.some(span => covers(span, weekday, minute))
}

type Translate = (key: string, params?: Record<string, unknown>) => string

const DAY_KEYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] as const

/** "Mo–So 22:00–06:00": the days as a range where they run on, the times as the clock shows them. */
export function describeSpan(span: WeekWindow, t: Translate): string {
  const days = DAY_KEYS.map((_, day) => day).filter(day => (span.days & (1 << day)) !== 0)
  const label = (day: number) => t(`common.weekdays.${DAY_KEYS[day]}`)
  const first = days[0]
  const last = days[days.length - 1]
  const contiguous = first !== undefined && last !== undefined && last - first === days.length - 1
  const dayText = contiguous && days.length > 2
    ? `${label(first)}–${label(last)}`
    : days.map(label).join(', ')
  return `${dayText} ${timeOf(span.start_minute)}–${timeOf(span.end_minute)}`
}

/** Every span of a window, one after the other; empty for a window without spans. */
export function describeWindow(window: DownloadWindow, t: Translate): string {
  return (window.windows ?? []).map(span => describeSpan(span, t)).join(' · ')
}
