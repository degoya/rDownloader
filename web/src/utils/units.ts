import { i18n } from '@/i18n'

/** IEC (1024) and SI (1000) unit ladders; which one is used is a settings choice. */
export const BINARY_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'] as const
export const DECIMAL_UNITS = ['B', 'kB', 'MB', 'GB', 'TB', 'PB'] as const

const UNITS = new Set<string>([...BINARY_UNITS, ...DECIMAL_UNITS])

/** Names already resolved, per language; a figure is formatted often and its unit never changes. */
const names = new Map<string, Map<string, string>>()

/**
 * What the interface language calls a unit: `MiB` is `Mio` in French, `MB` is `Mo` (RD-1480-16).
 *
 * The one source is `common.byte_units`, for figures and for the units attached to a field. A
 * rate (`MiB/s`) names its unit the same way, and anything that is no size (`min`, `s`) is
 * returned as it came. Only a language whose own catalogue has the name is cached: until its
 * chunk is in, the name is the English fallback and not yet its own.
 */
export function unitName(unit: string): string {
  const rate = unit.endsWith('/s')
  const size = rate ? unit.slice(0, -2) : unit
  if (!UNITS.has(size)) return unit
  const name = sizeName(size)
  return rate ? `${name}/s` : name
}

function sizeName(size: string): string {
  const locale = i18n.global.locale.value
  const known = names.get(locale)?.get(size)
  if (known !== undefined) return known
  const key = `common.byte_units.${size.toLowerCase()}`
  const name = i18n.global.t(key)
  if (i18n.global.te(key, locale)) {
    const named = names.get(locale) ?? new Map<string, string>()
    names.set(locale, named.set(size, name))
  }
  return name
}
