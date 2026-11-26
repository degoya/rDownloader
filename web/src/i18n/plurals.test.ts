import { afterEach, beforeAll, describe, expect, it } from 'vitest'

import { i18n, setLocale } from '@/i18n'
import { loadEveryLocale } from '@/test/locales'

import { PLURAL_RULES, groupCounters } from './plurals'

beforeAll(loadEveryLocale)
afterEach(() => setLocale('en'))

describe('plural forms', () => {
  it('counts zero and one as singular in French, two and up as plural', async () => {
    await setLocale('fr')
    const files = (count: number) => i18n.global.t('history.list.files', { count }, count)
    expect([0, 1, 2].map(files)).toEqual(['0 fichier', '1 fichier', '2 fichiers'])
  })

  it('leaves zero plural elsewhere', async () => {
    await setLocale('de')
    expect(i18n.global.t('history.list.files', { count: 0 }, 0)).toBe('0 Dateien')
  })

  it('keeps the spelled-out zero of three forms in French', async () => {
    await setLocale('fr')
    const files = (count: number) => i18n.global.t('common.units.file', { count }, count)
    expect([0, 1, 2].map(files)).toEqual(['aucun fichier', '1 fichier', '2 fichiers'])
  })

  it('has a rule for the languages that need one only', () => {
    expect(Object.keys(PLURAL_RULES)).toEqual(['fr'])
  })
})

describe('large counters in texts', () => {
  it.each([
    ['en', '12,345 files'],
    ['de', '12.345 Dateien'],
    ['es', '12.345 archivos'],
    ['fr', '12\u202f345 fichiers']
  ])('are grouped the way %s groups them, in their plural form', async (locale, text) => {
    await setLocale(locale)
    expect(i18n.global.t('common.units.file', { count: 12345 }, 12345)).toBe(text)
    expect(i18n.global.t('common.units.file', { count: 12345 })).toBe(text)
    expect(i18n.global.t('common.units.file', 12345)).toBe(text)
  })

  it('follow the language a call names rather than the active one', () => {
    expect(i18n.global.t('common.units.file', { count: 12345 }, { locale: 'de' })).toBe('12.345 Dateien')
    expect(i18n.global.t('common.units.file', 12345, { locale: 'de', named: {} })).toBe('12.345 Dateien')
  })

  it('leave small counts and every other number as they are', () => {
    const format = () => 'grouped'
    expect(groupCounters([{ count: 999 }, 999], format)).toEqual([{ count: 999 }, 999])
    expect(groupCounters([{ port: 65535 }], format)).toEqual([{ port: 65535 }])
    expect(groupCounters([{ count: 3, total: 4000 }], format)).toEqual([{ count: 3, total: 'grouped' }, 3])
    expect(groupCounters(['a default'], format)).toEqual(['a default'])
    expect(groupCounters([], format)).toEqual([])
  })
})
