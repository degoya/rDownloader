import { ref } from 'vue'

/**
 * How byte counts are rendered, mirrored from the settings document.
 *
 * A `ref` rather than a plain variable so a computed that formats a size re-runs when the
 * preference changes — switching it in the settings updates every figure on screen without a
 * reload. Defaults to binary, which is what every version so far displayed.
 */
export type ByteDisplay = 'binary' | 'decimal'

export const byteDisplay = ref<ByteDisplay>('binary')

export function setByteDisplay(value: string | null | undefined): void {
  byteDisplay.value = value === 'decimal' ? 'decimal' : 'binary'
}

/**
 * Which magnitude sizes are printed in (RD-106-14).
 *
 * `auto` scales every value on its own, which is what every earlier version did and stays the
 * default. Any other value pins all of them to that step of the ladder, so a list of sizes
 * spanning several orders of magnitude can be read down the column instead of in the head.
 *
 * Named by magnitude, not by unit: which name the step carries — MiB or MB — is the separate
 * `byteDisplay` choice, and the two settings have to stay combinable.
 */
export type ByteUnit = 'auto' | 'byte' | 'kilo' | 'mega' | 'giga' | 'tera' | 'peta'

/** The pinnable magnitudes in ladder order; the index is the power the divisor is raised to. */
export const BYTE_UNIT_STEPS: readonly ByteUnit[] = ['byte', 'kilo', 'mega', 'giga', 'tera', 'peta']

export const byteUnit = ref<ByteUnit>('auto')

export function setByteUnit(value: string | null | undefined): void {
  byteUnit.value = BYTE_UNIT_STEPS.includes(value as ByteUnit) ? (value as ByteUnit) : 'auto'
}

/** The ladder index a pinned unit stands for, or `null` while sizes scale on their own. */
export function fixedUnitIndex(): number | null {
  const index = BYTE_UNIT_STEPS.indexOf(byteUnit.value)
  return index < 0 ? null : index
}
