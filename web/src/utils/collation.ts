import { i18n } from '@/i18n'

/**
 * How texts sort in the interface language, not the browser's (RD-1480-16). Building a collator
 * is what costs, so there is one per language; read inside a `computed`, a language switch sorts
 * again.
 */
function comparator(options?: Intl.CollatorOptions): (left: string, right: string) => number {
  const collators = new Map<string, Intl.Collator>()
  return (left, right) => {
    const locale = i18n.global.locale.value
    let collator = collators.get(locale)
    if (!collator) {
      collator = new Intl.Collator(locale, options)
      collators.set(locale, collator)
    }
    return collator.compare(left, right)
  }
}

/** Alphabetically: `values.sort(compareText)`. */
export const compareText = comparator()

/** Numbers by their value and neither case nor accents apart: `2` before `10`. */
export const compareNatural = comparator({ numeric: true, sensitivity: 'base' })
