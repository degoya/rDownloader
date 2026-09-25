import { describe, expect, it } from 'vitest'

import { SUPPORTED_LOCALES, i18n } from '@/i18n'
import { translateServerMessage } from '@/i18n/server'

function flatten(value: unknown, prefix = ''): Record<string, string> {
  if (typeof value !== 'object' || value === null) return { [prefix]: String(value) }
  return Object.entries(value as Record<string, unknown>).reduce<Record<string, string>>((acc, [key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key
    return Object.assign(acc, flatten(child, path))
  }, {})
}

const placeholders = (text: string): string[] => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1] ?? '').sort()

describe('server failure codes', () => {
  // Codes are dotted (`plugin.timeout`) and stored as literal keys inside `codes`, which
  // vue-i18n's default resolver cannot reach — every backend message would silently fall
  // back to its English text.
  it('resolves a dotted code through the literal key it is stored under', () => {
    i18n.global.locale.value = 'en'
    expect(i18n.global.te('server.codes.plugin.timeout')).toBe(true)
    expect(i18n.global.t('server.codes.plugin.timeout')).not.toBe('server.codes.plugin.timeout')
  })

  it('translates a coded message instead of echoing the server text', () => {
    const catalogue = i18n.global.getLocaleMessage('de') as unknown as {
      server: { codes: Record<string, string | undefined> }
    }
    const expected = catalogue.server.codes['plugin.timeout']
    expect(expected).toBeTypeOf('string')

    i18n.global.locale.value = 'de'
    const german = translateServerMessage({ code: 'plugin.timeout', message: 'English fallback' })
    i18n.global.locale.value = 'en'

    expect(german).not.toBe('English fallback')
    expect(german).toBe(expected)
  })

  // The backend is English-only, so a capability travels as a stable identifier
  // (`media_download`). Interpolating it raw would put that identifier in front of the
  // reader in every language, which defeats the point of naming it at all (RD-102-03).
  it('names the affected capability in the reader\u2019s language', () => {
    i18n.global.locale.value = 'de'
    const german = translateServerMessage({
      code: 'media.tool_incompatible',
      message: 'yt-dlp 2019.01.01 is not compatible with media_download',
      params: { tool: 'yt-dlp', version: '2019.01.01', capability: 'media_download' }
    })
    i18n.global.locale.value = 'en'

    expect(german).toContain('yt-dlp')
    expect(german).toContain('Mediendownloads')
    expect(german).not.toContain('media_download')
  })

  // A code whose text names a number has to carry both forms. `translateServerMessage` passes a
  // numeric `count` to vue-i18n as the plural choice, so a single-form message would read
  // "1 unfinished downloads" to the one person most likely to see it (RD-108-10).
  it.each(['en', 'de', 'es', 'fr'])('%s pluralises the blocked plugin version', (locale) => {
    i18n.global.locale.value = locale as 'en'
    const one = translateServerMessage({
      code: 'plugin.version_in_use',
      message: 'English fallback',
      params: { count: '1', names: 'archive.bin' }
    })
    const many = translateServerMessage({
      code: 'plugin.version_in_use',
      message: 'English fallback',
      params: { count: '3', names: 'archive.bin, film.mkv' }
    })
    i18n.global.locale.value = 'en'

    expect(one).not.toBe('English fallback')
    expect(one).toContain('archive.bin')
    expect(one).not.toContain('1 ')
    expect(many).toContain('3')
    expect(many).toContain('film.mkv')
    expect(many).not.toBe(one)
  })
})

describe('locale catalogues', () => {
  const english = flatten(i18n.global.getLocaleMessage('en'))

  it('has messages for every supported locale', () => {
    for (const locale of SUPPORTED_LOCALES) {
      expect(Object.keys(flatten(i18n.global.getLocaleMessage(locale))).length).toBeGreaterThan(0)
    }
  })

  it.each(SUPPORTED_LOCALES.filter(locale => locale !== 'en'))('%s has exactly the English key set', (locale) => {
    const flat = flatten(i18n.global.getLocaleMessage(locale))
    const missing = Object.keys(english).filter(key => !(key in flat))
    const extra = Object.keys(flat).filter(key => !(key in english))
    expect({ missing, extra }).toEqual({ missing: [], extra: [] })
  })

  it.each(SUPPORTED_LOCALES)('%s has no empty values and matching placeholders', (locale) => {
    const flat = flatten(i18n.global.getLocaleMessage(locale))
    const empty = Object.entries(flat).filter(([, text]) => text.trim() === '').map(([key]) => key)
    expect(empty).toEqual([])
    const mismatched = Object.entries(flat)
      .filter(([key, text]) => key in english && placeholders(text).join(',') !== placeholders(english[key] ?? '').join(','))
      .map(([key]) => key)
    expect(mismatched).toEqual([])
  })
})

describe('keys referenced by components', () => {
  // The parity test above only compares the catalogues against each other, so a key that no
  // catalogue has — `common.cancel` instead of `common.actions.cancel` — slipped through and
  // rendered the raw key as a button label. This resolves what the templates actually ask for.
  const sources = import.meta.glob('@/{views,components}/**/*.vue', { eager: true, query: '?raw', import: 'default' })

  /** Literal `t('…')` / `$t("…")` calls; interpolated keys are skipped on purpose. */
  function literalKeys(source: string): string[] {
    return [...source.matchAll(/\$?\bt\(\s*'([a-z][\w.]*)'/g)].map(match => match[1] ?? '')
      .concat([...source.matchAll(/\$?\bt\(\s*"([a-z][\w.]*)"/g)].map(match => match[1] ?? ''))
  }

  it('resolves every literal translation key used in a view or component', () => {
    i18n.global.locale.value = 'en'
    const missing: string[] = []
    for (const [path, source] of Object.entries(sources)) {
      for (const key of literalKeys(source as string)) {
        if (!i18n.global.te(key)) missing.push(`${path}: ${key}`)
      }
    }
    expect(missing).toEqual([])
  })
})
