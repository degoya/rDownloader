/**
 * One term per concept and language, and one kind of quotation mark (RD-1480-15, `design.md`).
 *
 * The glossary of `design.md` as far as a word can be recognised. A search keyword may name the
 * other word as a synonym; that is the one catalogue part left out.
 */
import { describe, expect, it } from 'vitest'

import { REQUIRED_LOCALES, catalogueStrings } from '@/test/catalogues'

/** The word that is not used, and the one that is. */
const TERMS: Readonly<Record<string, readonly (readonly [RegExp, string])[]>> = {
  de: [
    [/kennwort/iu, 'Passwort'],
    [/spiegel(?!ung)/iu, 'Mirror'],
    [/(?<!zu|ver|ab|be)sicherung/iu, 'Backup'],
    [/(?<!Voll-)(?<![\p{L}-])vollbackup/iu, 'Voll-Backup'],
    [/(?<!\p{L})accounts?(?!\p{L})/iu, 'Konto'],
    [/storage-root|secret-store|(?<!\p{L})vault/iu, 'Speicherort, Tresor'],
    [/api-key/iu, 'API-Schlüssel'],
    [/browsererweiterung/iu, 'Browser-Erweiterung'],
    [/automatisierung/iu, 'Automation'],
    [/abonnement/iu, 'Abo'],
    [/download-liste/iu, 'Downloadliste'],
    [/(?<!\p{L})freien? download/iu, 'Gratis-Download'],
    [/(?<!\p{L})(der|den|diese[rn]|eine[nr]|jede[nr]) token(?!\p{L})/iu, 'das Token']
  ],
  fr: [
    [/passkey/iu, 'clé d’accès'],
    [/clés? (d['’]|de l['’])API/iu, 'clé API']
  ],
  es: [
    [/passkey/iu, 'clave de acceso'],
    [/posprocesamiento/iu, 'posprocesado'],
    [/(?<!\p{L})(de medios|Medios)(?!\p{L})/u, 'multimedia'],
    [/alojamiento/iu, 'hoster']
  ]
}

describe.each(Object.keys(TERMS))('%s terms', (locale) => {
  it('uses one word for one thing', () => {
    const strings = [...catalogueStrings(locale)].filter(([key]) => !key.startsWith('nav:search.keywords'))
    const offenders = strings.flatMap(([key, text]) =>
      (TERMS[locale] ?? []).filter(([word]) => word.test(text.replace(/\{\w+\}/gu, ''))).map(([, term]) => `${key}: ${term}: ${text}`))
    expect(offenders).toEqual([])
  })
})

/** Straight quotation marks that are the thing named: an argument's own, an attribute, the sign. */
const QUOTED: readonly string[] = [
  'server:codes.extract.password_has_quote',
  'siterules:bundled.archive-org-items',
  'subscriptions:form.script_arguments_description'
]
const PAIRS: Readonly<Record<string, readonly [string, string]>> = { de: ['„', '“'], en: ['“', '”'], fr: ['«', '»'], es: ['«', '»'] }

describe.each(REQUIRED_LOCALES)('%s quotation marks', (locale) => {
  it('are the language’s own, and in pairs', () => {
    const [open, close] = PAIRS[locale] ?? ['', '']
    const foreign = ['"', '„', '“', '”', '«', '»'].filter(mark => mark !== open && mark !== close)
    const count = (text: string, mark: string): number => text.split(mark).length - 1
    const offenders = [...catalogueStrings(locale)].filter(([key, text]) =>
      !QUOTED.includes(key) && (foreign.some(mark => text.includes(mark)) || count(text, open) !== count(text, close)))
    expect(offenders).toEqual([])
  })
})
