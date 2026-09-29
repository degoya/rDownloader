/**
 * A whole number above zero from a form field, or `null` when the field is empty or holds
 * anything else. Nuxt UI's `UInput type="number"` hands its model a number, a text field a
 * string; both arrive here.
 */
export function positiveCount(value: string | number | null | undefined): number | null {
  const parsed = typeof value === 'number' ? value : Number.parseInt(String(value ?? '').trim(), 10)
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null
}
