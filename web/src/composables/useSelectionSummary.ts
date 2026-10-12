import { useI18n } from 'vue-i18n'

import type { SelectionParts } from '@/utils/selectionSize'

/** vue-i18n's `t` as the wording below calls it. */
export type Translate = (key: string, named: Record<string, unknown>, plural?: number) => string

/**
 * What a selection holds, packages and files apart (RD-1260-07): "4 packages · 11 files", or "3
 * files in 1 package" while one package holds them all; the LinkGrabber's links likewise, its NZB
 * imports after them. A package counts as soon as one of its rows is ticked (`SelectionParts`).
 */
export function selectionFigures(t: Translate, parts: SelectionParts): string {
  const figures: string[] = []
  if (parts.items) {
    const items = t(`common.units.${parts.unit}`, { count: parts.items }, parts.items)
    const packages = t('common.units.package', { count: parts.packages }, parts.packages)
    if (parts.packages === 1) figures.push(t('common.selection.detail', { items, packages }))
    else figures.push(...(parts.packages ? [packages] : []), items)
  }
  if (parts.imports) figures.push(t('common.units.nzb_import', { count: parts.imports }, parts.imports))
  return figures.join(' · ')
}

/** "4 packages · 11 files selected": the selection as the list bar, the selection bar and the status bar name it. */
export function selectionSummary(t: Translate, parts: SelectionParts): string {
  return t('common.selection.summary', { figures: selectionFigures(t, parts) })
}

/** `selectionSummary` in the calling component's language. */
export function useSelectionSummary(): (parts: SelectionParts) => string {
  const { t } = useI18n()
  const translate: Translate = (key, named, plural) => plural === undefined ? t(key, named) : t(key, named, plural)
  return parts => selectionSummary(translate, parts)
}
