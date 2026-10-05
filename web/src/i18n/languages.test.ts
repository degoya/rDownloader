/**
 * The language list against the catalogues that have to follow it (RD-1100-09).
 *
 * `web/src/locales/languages.json` is the one place a language is named. These cases hold every
 * catalogue tree to it: the web interface's, the browser extension's and each bundled plugin's.
 * A required language is complete everywhere — the per-catalogue tests compare its keys — and an
 * in-progress one may be partial or absent outside the web interface, falling back to English.
 */
import { existsSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { afterEach, describe, expect, it } from 'vitest'

import { LANGUAGES, REQUIRED_LOCALES, i18n } from '@/i18n'

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const webLocales = join(repositoryRoot, 'web/src/locales')
const extensionLocales = join(repositoryRoot, 'extension/_locales')
const plugins = join(repositoryRoot, 'plugins')

function directories(path: string): string[] {
  return readdirSync(path, { withFileTypes: true }).filter(entry => entry.isDirectory()).map(entry => entry.name).sort()
}

const listed = LANGUAGES.map(language => language.code)

describe('the language list', () => {
  it('names each language once, by two lowercase letters, with its own name and a known status', () => {
    expect(new Set(listed).size).toBe(listed.length)
    for (const { code, name, status } of LANGUAGES) {
      expect(code).toMatch(/^[a-z]{2}$/)
      expect(name.trim()).not.toBe('')
      expect(['required', 'in-progress']).toContain(status)
    }
  })

  it('keeps English and the four languages the project ships required', () => {
    expect([...REQUIRED_LOCALES].sort()).toEqual(expect.arrayContaining(['de', 'en', 'es', 'fr']))
  })

  it('has a web catalogue directory for every language and no directory it does not name', () => {
    expect(directories(webLocales)).toEqual([...listed].sort())
  })

  it('gives every required language every catalogue file English has', () => {
    const english = readdirSync(join(webLocales, 'en')).sort()
    for (const locale of REQUIRED_LOCALES) {
      expect({ locale, files: readdirSync(join(webLocales, locale)).sort() }).toEqual({ locale, files: english })
    }
  })
})

describe('the extension and plugin catalogues', () => {
  // The extension's own test compares the keys; here only which languages exist.
  it('ships the extension in every required language and in no language the list lacks', () => {
    const shipped = directories(extensionLocales)
    expect(shipped.filter(code => !listed.includes(code))).toEqual([])
    expect(REQUIRED_LOCALES.filter(code => !shipped.includes(code))).toEqual([])
  })

  // `crates/rd-plugin-host/tests/bundled_locales.rs` compares each file's codes with English.
  it('gives every bundled plugin a file per required language and none for a language the list lacks', () => {
    const problems: string[] = []
    for (const plugin of directories(plugins)) {
      const dir = join(plugins, plugin, 'locales')
      if (!existsSync(join(plugins, plugin, 'manifest.toml')) || !existsSync(dir)) continue
      const shipped = readdirSync(dir).filter(name => name.endsWith('.json')).map(name => name.slice(0, -'.json'.length))
      for (const code of shipped.filter(code => !listed.includes(code))) problems.push(`${plugin}: ${code}.json is not in the list`)
      for (const code of REQUIRED_LOCALES.filter(code => !shipped.includes(code))) problems.push(`${plugin}: no ${code}.json`)
    }
    expect(problems).toEqual([])
  })
})

describe('a key a language does not have yet', () => {
  afterEach(() => {
    i18n.global.locale.value = 'en'
  })

  // `zz` stands in for an unfinished language: one key translated, the rest missing.
  it('shows English, never the raw key', () => {
    i18n.global.mergeLocaleMessage('zz', { common: { actions: { cancel: 'Zz-cancel' } } })
    i18n.global.locale.value = 'zz'

    expect(i18n.global.t('common.actions.cancel')).toBe('Zz-cancel')
    const english = i18n.global.t('common.actions.save', {}, { locale: 'en' })
    expect(english).not.toBe('common.actions.save')
    expect(i18n.global.t('common.actions.save')).toBe(english)
    expect(i18n.global.t('server.codes.plugin.timeout')).toBe(i18n.global.t('server.codes.plugin.timeout', {}, { locale: 'en' }))
  })
})
