/**
 * How a count picks its form of a piped message, and how the count itself is printed.
 *
 * Its own module for the reason `formats.ts` is one: the component-test harness registers the
 * same rules, and tests mock `@/i18n` wholesale.
 */

type PluralRule = (choice: number, choicesLength: number, original?: PluralRule) => number

/**
 * vue-i18n's default is English's: zero takes the plural. In French zero and everything below
 * two take the singular (CLDR `one`: i = 0, 1) — "0 fichier". A message of three forms spells
 * its zero out ("aucun fichier | …") and keeps the default's none/one/many.
 */
export const PLURAL_RULES: Record<string, PluralRule> = {
  fr: (choice, choicesLength, original) => {
    const count = Math.abs(choice)
    if (choicesLength === 2) return count < 2 ? 0 : 1
    return original ? original(choice, choicesLength) : Math.min(Math.floor(count), choicesLength - 1)
  }
}

/** The named values that count things; a port, a year or a version is none of them. */
const COUNTERS = ['count', 'total'] as const

/** Below this no language of the list groups, so the arguments stay as they are. */
const GROUPED_FROM = 1000

type Values = Record<string, unknown>

function isValues(value: unknown): value is Values {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isLarge(value: unknown): value is number {
  return typeof value === 'number' && Math.abs(value) >= GROUPED_FROM
}

/** `named` with its large counters as text; the same object when there is none. */
function grouped(named: Values, format: (value: number) => string): Values {
  const large = COUNTERS.filter(name => isLarge(named[name]))
  if (!large.length) return named
  return { ...named, ...Object.fromEntries(large.map(name => [name, format(named[name] as number)])) }
}

/**
 * The arguments of a `t()` call after its key, with a large `{count}` or `{total}` grouped the
 * way the language groups it: "12,345 entries", "12.345 Einträge", "12 345 entrées" (RD-1480-16).
 *
 * vue-i18n prints a number as it is and has no hook for it, and some two hundred calls pass a
 * count. A count that becomes text no longer picks the plural form, so the number goes along as
 * the call's plural.
 */
export function groupCounters(args: readonly unknown[], format: (value: number, locale?: string) => string): unknown[] {
  const [first, second] = args
  const options = isValues(second) ? second : undefined
  const inLocale = (value: number): string => format(value, typeof options?.locale === 'string' ? options.locale : undefined)
  if (typeof first === 'number') {
    if (!isLarge(first)) return [...args]
    return [first, { ...options, named: grouped({ count: first, ...(isValues(options?.named) ? options.named : {}) }, inLocale) }]
  }
  if (!isValues(first)) return [...args]
  const named = grouped(first, inLocale)
  if (named === first) return [...args]
  if (typeof first.count !== 'number') return [named, ...args.slice(1)]
  if (options) return [named, { ...options, plural: first.count }]
  return [named, typeof second === 'string' ? { default: second, plural: first.count } : first.count]
}
