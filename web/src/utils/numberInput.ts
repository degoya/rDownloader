/**
 * What every `UInputNumber` of the interface is told about its number (RD-1110-10).
 *
 * The field formats and parses in the interface language (`UApp`'s locale), so German and French
 * read and type a decimal comma. Without `format-options` it keeps up to three decimals and
 * groups thousands; each kind of number below says what it allows instead. A field with decimals
 * also turns `step-snapping` off: with the default step of 1 the field would round 1,5 to 2.
 */

/** Counts, seconds, hours, days: a typed 1,5 becomes 2, never a fraction the API refuses. */
export const WHOLE: Intl.NumberFormatOptions = { maximumFractionDigits: 0 }

/** Ports, versions and priorities: whole, and without a thousands separator (65535, not 65.535). */
export const PLAIN: Intl.NumberFormatOptions = { maximumFractionDigits: 0, useGrouping: false }

/** Sizes in MiB or GiB: two decimals, as fine as `byteModel` rounds. */
export const DECIMAL: Intl.NumberFormatOptions = { maximumFractionDigits: 2 }

/** Seed ratios: three decimals, as fine as the API's `ratio_milli` stores them. */
export const RATIO: Intl.NumberFormatOptions = { maximumFractionDigits: 3 }

/**
 * The value of an optional field. `UInputNumber` reports a field the person emptied as
 * `undefined`; the DTOs say "not set" with `null`, and a PUT that dropped the key would mean
 * something else to an endpoint that keeps what it is not sent.
 */
export function orNull(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** Whether an obligatory field holds a number; an emptied one blocks its form's save. */
export function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}
