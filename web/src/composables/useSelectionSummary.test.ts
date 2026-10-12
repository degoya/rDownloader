/**
 * The selection named with packages and files apart (RD-1260-07, owner 2026-10-12): "4 packages ·
 * 11 files selected", "3 files in 1 package" while one package holds them, the LinkGrabber's links
 * and NZB imports alike, each figure in its plural in all four languages.
 */
import { describe, expect, it } from 'vitest'

import de from '@/locales/de/common.json'
import en from '@/locales/en/common.json'
import es from '@/locales/es/common.json'
import fr from '@/locales/fr/common.json'
import { createTestI18n } from '@/test/mount'
import type { SelectionParts } from '@/utils/selectionSize'

import { selectionSummary, type Translate } from './useSelectionSummary'

const CATALOGUES = { de, en, es, fr }

function summary(locale: keyof typeof CATALOGUES, parts: SelectionParts): string {
  const { t } = createTestI18n({ common: CATALOGUES[locale] }, locale).global
  const translate: Translate = (key, named, plural) => plural === undefined ? t(key, named) : t(key, named, plural)
  return selectionSummary(translate, parts)
}

const files = (packages: number, items: number): SelectionParts => ({ packages, items, unit: 'file', imports: 0 })

describe('selectionSummary', () => {
  it('names packages and files apart', () => {
    expect(summary('en', files(4, 11))).toBe('4 packages · 11 files selected')
    expect(summary('de', files(4, 11))).toBe('4 Pakete · 11 Dateien ausgewählt')
    expect(summary('es', files(4, 11))).toBe('Selección: 4 paquetes · 11 archivos')
    expect(summary('fr', files(4, 11))).toBe('Sélection : 4 paquets · 11 fichiers')
  })

  it('says "in 1 package" while one package holds every ticked file', () => {
    expect(summary('en', files(1, 3))).toBe('3 files in 1 package selected')
    expect(summary('de', files(1, 3))).toBe('3 Dateien in 1 Paket ausgewählt')
    expect(summary('es', files(1, 1))).toBe('Selección: 1 archivo en 1 paquete')
    expect(summary('fr', files(1, 1))).toBe('Sélection : 1 fichier dans 1 paquet')
  })

  it('takes the singular and the plural of each figure on its own', () => {
    expect(summary('de', files(2, 1))).toBe('2 Pakete · 1 Datei ausgewählt')
    expect(summary('en', files(2, 2))).toBe('2 packages · 2 files selected')
  })

  it('counts the LinkGrabber\'s links the same way, its NZB imports after them', () => {
    expect(summary('en', { packages: 3, items: 4, unit: 'link', imports: 1 })).toBe('3 packages · 4 links · 1 NZB import selected')
    expect(summary('de', { packages: 1, items: 2, unit: 'link', imports: 2 })).toBe('2 Links in 1 Paket · 2 NZB-Importe ausgewählt')
    expect(summary('fr', { packages: 0, items: 0, unit: 'link', imports: 3 })).toBe('Sélection : 3 imports NZB')
  })
})
