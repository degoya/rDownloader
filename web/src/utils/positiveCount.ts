/**
 * A whole number above zero from a form field, or `null` when the field is empty or holds
 * anything else. Nuxt UI's `UInputNumber` hands its model a number, or `undefined` once emptied,
 * a text field a string; all of them arrive here.
 */
export function positiveCount(value: string | number | null | undefined): number | null {
  const parsed = typeof value === 'number' ? value : Number.parseInt(String(value ?? '').trim(), 10)
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null
}
