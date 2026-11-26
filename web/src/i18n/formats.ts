/**
 * The number and date shapes the interface renders in, named rather than spelled out at the
 * call site.
 *
 * Its own module, not part of `i18n/index.ts`, because the component-test harness registers the
 * same shapes and several tests mock `@/i18n` wholesale. A harness that reaches into a mocked
 * module fails every one of them for a reason that has nothing to do with what they check.
 *
 * `d(value, 'long')` was in use in two places before `long` existed here, and vue-i18n answers an
 * unregistered format name with an empty string rather than a complaint — so both rendered
 * nothing at all where a timestamp belonged. Every name a template or `utils/format` passes must
 * have a row here.
 */
export const NUMBER_FORMATS: Record<string, Intl.NumberFormatOptions> = {
  decimal: { style: 'decimal', maximumFractionDigits: 1 },
  integer: { style: 'decimal', maximumFractionDigits: 0 },
  percent: { style: 'percent', maximumFractionDigits: 0 },
  // A measured figure with exactly so many decimals and no thousands separator, what `toFixed`
  // printed: the columns are as wide as `1023 MiB`, not `1.023 MiB` (RD-1480-16).
  fixed0: { style: 'decimal', minimumFractionDigits: 0, maximumFractionDigits: 0, useGrouping: false },
  fixed1: { style: 'decimal', minimumFractionDigits: 1, maximumFractionDigits: 1, useGrouping: false },
  fixed2: { style: 'decimal', minimumFractionDigits: 2, maximumFractionDigits: 2, useGrouping: false },
  fixed3: { style: 'decimal', minimumFractionDigits: 3, maximumFractionDigits: 3, useGrouping: false }
}

export const DATETIME_FORMATS: Record<string, Intl.DateTimeFormatOptions> = {
  short: { dateStyle: 'medium', timeStyle: 'short' },
  long: { dateStyle: 'long', timeStyle: 'short' },
  date: { dateStyle: 'medium' },
  time: { timeStyle: 'short' }
}
