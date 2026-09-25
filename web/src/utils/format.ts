import { computed, type WritableComputedRef } from 'vue'

import type { BadgeProps } from '@nuxt/ui'

import type { Download, DownloadPriority, DownloadState, PostprocessLevel, PostprocessStage } from '@/api/types'
import { i18n } from '@/i18n'
import { byteDisplay, fixedUnitIndex } from '@/utils/byteDisplay'

/** The two step sizes a form ever edits a byte count in. */
export const MIB = 1024 ** 2
export const GIB = 1024 ** 3

/**
 * A byte count the API carries as a string, bound to an input that edits MiB or GiB.
 *
 * Six components wrote this getter/setter pair out by hand, in three spellings of the same
 * constant (`1024 ** 3`, `1024 * 1024`, `/ 1024 / 1024`) and with three different answers to a
 * cleared field. Only the last of those is a real difference, and it is not a free choice: a
 * field the API types as nullable clears to `null`, one it types as an obligatory string clears
 * to `'0'`. That is what `empty` says, and it decides what an absent value reads back as too.
 *
 * The getter rounds to two decimals. None of the hand-written copies did, so dividing a stored
 * 3 221 225 472 by a GiB could put `3.0000000000000004` in the field; two decimals is finer than
 * any of the steps these inputs use.
 */
export function byteModel(
  read: () => string | null | undefined,
  write: (raw: string | null) => void,
  factor: number,
  empty: string | null = null
): WritableComputedRef<number | null> {
  const blank = empty === null ? null : 0
  return computed<number | null>({
    get: () => {
      const raw = read()
      if (raw === null || raw === undefined || raw === '') return blank
      const scaled = Number(raw) / factor
      return Number.isFinite(scaled) ? Math.round(scaled * 100) / 100 : blank
    },
    set: (value) => {
      const scaled = Number(value)
      write(Number.isFinite(scaled) && scaled > 0 ? String(Math.round(scaled * factor)) : empty)
    }
  })
}

/** IEC (1024) and SI (1000) unit ladders; which one is used is a settings choice. */
const BINARY_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'] as const
const DECIMAL_UNITS = ['B', 'kB', 'MB', 'GB', 'TB', 'PB'] as const

function toBytes(value: string | bigint): bigint | null {
  try {
    return typeof value === 'bigint' ? value : BigInt(value)
  } catch {
    return null
  }
}

/** Smallest figure a pinned unit prints; below it the digits would all be zero. */
const FIXED_FLOOR = 0.001

/**
 * The digits a value keeps once its unit is pinned.
 *
 * Automatic scaling never needs this: it picks the step that keeps the number between one and
 * the divisor, so one decimal is always enough. A pinned unit has no such guarantee — the same
 * column holds 4 GiB and 4 KiB — so the rule is "roughly three significant digits", and a value
 * too small for even three decimals prints `<0.001` rather than a `0.000` that reads as nothing
 * at all. `0.004 GB` helps nobody, but `0.000 GB` for a real file helps less.
 */
function fixedValue(scaled: number): string {
  if (scaled === 0) return '0'
  if (scaled >= 100) return scaled.toFixed(0)
  if (scaled >= 10) return scaled.toFixed(1)
  if (scaled >= 1) return scaled.toFixed(2)
  return scaled < FIXED_FLOOR ? `<${FIXED_FLOOR.toFixed(3)}` : scaled.toFixed(3)
}

/** Splits a byte count into its scaled number and unit so callers can share a unit label. */
function scaleBytes(bytes: bigint): { value: string, unit: string } {
  const decimal = byteDisplay.value === 'decimal'
  const units = decimal ? DECIMAL_UNITS : BINARY_UNITS
  const divisor = decimal ? 1000 : 1024
  let scaled = Number(bytes)
  // A pinned magnitude short-circuits the ladder walk: every value lands on the same step, which
  // is the whole point of the setting (RD-106-14).
  const fixed = fixedUnitIndex()
  if (fixed !== null) {
    const index = Math.min(fixed, units.length - 1)
    return { value: fixedValue(scaled / divisor ** index), unit: units[index] ?? 'B' }
  }
  let unit = 0
  while (scaled >= divisor && unit < units.length - 1) {
    scaled /= divisor
    unit += 1
  }
  return { value: scaled >= 100 || unit === 0 ? scaled.toFixed(0) : scaled.toFixed(1), unit: units[unit] ?? 'B' }
}

export function formatBytes(value: string | bigint | null | undefined): string {
  if (value === null || value === undefined) {
    return i18n.global.t('common.values.unknown')
  }
  const bytes = toBytes(value)
  if (bytes === null) return '—'
  const scaled = scaleBytes(bytes)
  return `${scaled.value} ${scaled.unit}`
}

/**
 * Renders "done / total" compactly: the unit is printed once when both values share it.
 *
 * Checkpointed `committed_bytes` can overshoot `total_bytes`, so the done value is clamped
 * to the total – the same guarantee `progressOf` gives for the progress bar.
 */
export function formatByteProgress(
  committed: string | bigint | null | undefined,
  total: string | bigint | null | undefined
): string {
  if (total === null || total === undefined) return formatBytes(committed)
  const totalBytes = toBytes(total)
  const committedBytes = committed === null || committed === undefined ? null : toBytes(committed)
  if (totalBytes === null || totalBytes === 0n) return formatBytes(committed)
  if (committedBytes === null) return `${formatBytes(committed)} / ${formatBytes(total)}`
  const done = scaleBytes(committedBytes > totalBytes ? totalBytes : committedBytes)
  const all = scaleBytes(totalBytes)
  return done.unit === all.unit ? `${done.value} / ${all.value} ${all.unit}` : `${done.value} ${done.unit} / ${all.value} ${all.unit}`
}

export function formatRate(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond === null || bytesPerSecond === undefined || !Number.isFinite(bytesPerSecond)) {
    return '— B/s'
  }
  return `${formatBytes(BigInt(Math.max(0, Math.round(bytesPerSecond))))}/s`
}

/**
 * Renders a span of seconds as `m:ss`, or `h:mm:ss` from an hour up.
 *
 * Empty for anything there is nothing to say about — no value, a negative one, or one that is
 * not a finite number. Callers print the empty string, which is the point: a remaining time
 * that cannot be measured shows nothing rather than a placeholder.
 */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds <= 0) return ''
  const total = Math.round(seconds)
  const minutes = Math.floor(total / 60) % 60
  const hours = Math.floor(total / 3600)
  const pad = (value: number): string => String(value).padStart(2, '0')
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(total % 60)}`
    : `${minutes}:${pad(total % 60)}`
}

export function progressOf(download: Download): number {
  if (!download.total_bytes || download.total_bytes === '0') {
    return 0
  }
  const total = Number(download.total_bytes)
  const committed = Number(download.committed_bytes)
  return Number.isFinite(total) && total > 0 ? Math.min(100, (committed / total) * 100) : 0
}

/** Archive volumes the post-processor can unpack (`.rar`, `.r01`, `.7z.001`, …). */
const ARCHIVE_NAME = /\.(zip|7z|rar|r\d{2}|z\d{2}|\d{3})$/i

/** A finished file that the extractor can act on – the gate for every "extract" action. */
function isExtractable(download: Pick<Download, 'state' | 'file_name'>): boolean {
  return download.state === 'completed' && ARCHIVE_NAME.test(download.file_name)
}

export function hasExtractable(downloads: readonly Pick<Download, 'state' | 'file_name'>[]): boolean {
  return downloads.some(isExtractable)
}

/** A PAR2 index or recovery volume by name, the rule `rd_core::is_recovery_volume` applies. */
const PAR2_NAME = /\.par2$/i

/**
 * PAR2 repair data rather than payload (RD-107-10).
 *
 * The server marks the row when the NZB is queued and again once the real name is known
 * (RD-108-23). A `true` is final. A `false` is not: it was taken on the name the row had at
 * the time, which for an obfuscated post is the raw subject line, so a name that says PAR2
 * now outranks it. A volume that never arrived is not a defect by itself: unless the payload
 * is short, nobody asked for it.
 */
export function isRecoveryVolume(download: Pick<Download, 'file_name'>): boolean {
  // The generated client type grows `recovery` with the next contract run; the intersection
  // keeps this readable both before and after that.
  const marked = (download as Pick<Download, 'file_name'> & { recovery?: boolean }).recovery
  return marked === true || PAR2_NAME.test(download.file_name)
}

/**
 * A timestamp in the reader's language, or `''` when there is none to show.
 *
 * Eight components carried a private copy of this — under three names, `formatDate`,
 * `formatDateTime` and `formatMoment` — and the copies disagreed. Half passed the application's
 * locale to `toLocaleString`, half passed nothing and so rendered in the *browser's* language
 * instead; half guarded an unparsable value and half did not. `d()` has to be guarded either
 * way: vue-i18n throws on an invalid `Date` rather than returning anything.
 */
export function formatMoment(value: string | null | undefined): string {
  return formatAt(value, 'short')
}

/** The same, to the day: no clock where the time of day carries nothing. */
export function formatDay(value: string | null | undefined): string {
  return formatAt(value, 'date')
}

/** The same, spelled out, for the few places that name a single moment rather than list many. */
export function formatLongMoment(value: string | null | undefined): string {
  return formatAt(value, 'long')
}

function formatAt(value: string | null | undefined, format: 'short' | 'date' | 'long'): string {
  if (!value) return ''
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? '' : i18n.global.d(parsed, format)
}

/**
 * The lifecycle label of a row, in the reader's language.
 *
 * `skipped` carries two meanings and only one of them is a mirror (RD-120-16). A mirror is an
 * alternative source for the same bytes; a postponed PAR2 recovery volume is a statement about
 * *when*, not about redundancy — it is fetched only once the repair turns out to need it
 * (RD-107-04). The two are told apart by `mirror_group`, which is the same test the scheduler
 * makes (`rd_scheduler::control::stand_down_siblings_of`): every `skipped` mirror is written
 * with a group, the postponed volumes never are. Passing no row keeps the mirror wording, which
 * is what the callers that only have a state mean.
 */
export function stateLabel(state: DownloadState, download?: Pick<Download, 'mirror_group'>): string {
  if (state === 'skipped' && download && !download.mirror_group) {
    return i18n.global.t('common.states.skipped_postponed')
  }
  return i18n.global.t(`common.states.${state}`)
}

export function stateColor(state: DownloadState): BadgeProps['color'] {
  if (state === 'completed' || state === 'seeding') return 'success'
  if (state === 'failed' || state === 'cancelled') return 'error'
  if (state === 'blocked' || state === 'retry_wait') return 'warning'
  // A skipped row is waiting, not stopped and not broken; neutral says so. That holds for
  // both kinds: the mirror standing by and the postponed recovery volume.
  if (state === 'paused' || state === 'skipped') return 'neutral'
  return 'primary'
}

const PRIORITIES: DownloadPriority[] = ['high', 'normal', 'low']

/** Priority select items in the active language (call inside a `computed`). */
export function priorityItems(): { label: string, value: DownloadPriority }[] {
  return PRIORITIES.map(value => ({ label: i18n.global.t(`common.priority.${value}`), value }))
}

const POSTPROCESS_LEVELS: PostprocessLevel[] = ['none', 'repair', 'unpack', 'delete']

/** Sentinel select value for "inherit the category/global default". */
export const INHERIT_LEVEL = '__inherit__'

/** Post-processing level select items in the active language (call inside a `computed`). */
export function postprocessLevelItems(withInherit = true): { label: string, value: string }[] {
  const items = POSTPROCESS_LEVELS.map(value => ({ label: i18n.global.t(`downloads.postprocess.levels.${value}`), value: value as string }))
  return withInherit ? [{ label: i18n.global.t('downloads.postprocess.levels.inherit'), value: INHERIT_LEVEL }, ...items] : items
}

export function postprocessStageLabel(stage: PostprocessStage | null | undefined): string {
  return stage ? i18n.global.t(`downloads.postprocess.stages.${stage}`) : ''
}
