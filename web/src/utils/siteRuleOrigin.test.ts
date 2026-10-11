/**
 * The descriptions of the bundled site rules in every language (RD-1240-33).
 *
 * The examples are stored with the English text their file carries, so the German interface
 * showed English under every bundled rule. The list now reads a catalogue key per bundled rule;
 * this file holds the catalogues to the bundle: every rule the app brings has its description in
 * every required language, and the English one is the bundle's own text, so a description changed
 * in the bundle cannot leave the translations describing the old rule.
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { beforeAll, describe, expect, it } from 'vitest'

import { REQUIRED_LOCALES, i18n } from '@/i18n'
import { loadEveryLocale } from '@/test/locales'

import { bundledDescriptionKey } from './siteRuleOrigin'

beforeAll(loadEveryLocale)

const examplesFile = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../../crates/rd-siterules/resources/examples.json'
)

interface Bundle { rules: { rule: { id: string, description?: string } }[] }

/** The web package is tested inside the repository; without the crates there is nothing to read. */
const bundled: Bundle['rules'] = existsSync(examplesFile)
  ? (JSON.parse(readFileSync(examplesFile, 'utf8')) as Bundle).rules
  : []

describe('bundled site rule descriptions', () => {
  it.skipIf(bundled.length === 0)('has every bundled rule described in every required language', () => {
    for (const { rule } of bundled) {
      const key = `siterules.bundled.${rule.id}`
      for (const locale of REQUIRED_LOCALES) {
        expect(i18n.global.te(key, locale), `${key} in ${locale}`).toBe(true)
      }
      expect(i18n.global.t(key, {}, { locale: 'en' }), key).toBe(rule.description)
    }
  })

  it('reads the catalogue only while the rule is still the example', () => {
    expect(bundledDescriptionKey({ id: 'debian-cd', origin: { kind: 'example' } }))
      .toBe('siterules.bundled.debian-cd')
    for (const kind of ['editor', 'import', 'mcp', 'unknown'] as const) {
      expect(bundledDescriptionKey({ id: 'debian-cd', origin: { kind } }), kind).toBeNull()
    }
  })
})
