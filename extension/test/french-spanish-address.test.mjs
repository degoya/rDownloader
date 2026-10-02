// French says "vous", Spanish says "tú" (owner, 2026-10-02; `design.md`).
//
// The same heuristic as `web/src/i18n/frenchSpanishAddress.test.ts`, which explains the word
// lists: French "tu" pronouns, an elided "t'", irregular "tu" imperatives and a hyphenated
// imperative whose verb does not end in "z"; Spanish "usted", an accented reflexive "usted"
// imperative and a formal imperative at the start of a clause. A sentence the heuristic misreads
// gets its phrase listed in EXCEPTIONS by message key, as in `german-address.test.mjs`.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const locales = join(dirname(fileURLToPath(import.meta.url)), '..', '_locales')

const FRENCH_WORDS = new Set([
  'tu', 'toi', 'ton', 'ta', 'tes', 'te', 'tien', 'tienne', 'tiens', 'tiennes',
  'mets', 'fais', 'prends', 'reprends', 'attends', 'vois', 'lis', 'relis', 'écris'
])
const FRENCH_HYPHENATED = new Set(['toi', 'moi', 'en', 'le', 'la', 'les', 'y'])
const SPANISH_WORDS = new Set(['usted', 'ustedes'])
const SPANISH_IMPERATIVES = new Set([
  'abra', 'acepte', 'active', 'actualice', 'añada', 'apruebe', 'asigne', 'borre', 'cambie',
  'cancele', 'cierre', 'compare', 'compruebe', 'configure', 'confirme', 'conecte', 'consulte',
  'copie', 'cree', 'defina', 'deje', 'descargue', 'desactive', 'desmarque', 'detenga', 'ejecute',
  'elija', 'elimine', 'escanee', 'escriba', 'espere', 'guarde', 'haga', 'indique', 'inicie',
  'instale', 'intente', 'introduzca', 'marque', 'mueva', 'pegue', 'permita', 'ponga', 'pulse',
  'quite', 'rechace', 'recargue', 'reinicie', 'resuelva', 'responda', 'revise', 'seleccione',
  'use', 'utilice', 'vincule', 'vuelva'
])

/** Phrases the heuristic misreads, by locale, message key and the phrase that carries them. */
const EXCEPTIONS = { fr: {}, es: {} }

const words = (text) => text.split(/[^\p{L}\p{N}'’-]+/u).filter(Boolean)

function addressesAsTu(text) {
  return words(text).some((word) => {
    const lower = word.toLowerCase()
    if (FRENCH_WORDS.has(lower) || /^t['’]\p{L}/u.test(lower)) return true
    const parts = lower.split('-')
    const pronoun = parts.at(-1) ?? ''
    const verb = parts.at(-2) ?? ''
    return parts.length > 1 && FRENCH_HYPHENATED.has(pronoun) && verb !== '' && !verb.endsWith('z')
  })
}

function addressesAsUsted(text) {
  const formal = words(text).some((word) => {
    const lower = word.toLowerCase()
    return SPANISH_WORDS.has(lower) || (/[áéíóú]/u.test(lower) && /(ese|ase)$/u.test(lower))
  })
  return (
    formal ||
    text
      .split(/[.!?:;—–¿¡(«]/u)
      .some((clause) => SPANISH_IMPERATIVES.has(/^\s*(\p{L}+)/u.exec(clause)?.[1]?.toLowerCase() ?? ''))
  )
}

for (const [locale, offends, form] of [
  ['fr', addressesAsTu, 'tu'],
  ['es', addressesAsUsted, 'usted']
]) {
  test(`the ${locale} extension texts never address the reader as "${form}"`, () => {
    const messages = JSON.parse(readFileSync(join(locales, locale, 'messages.json'), 'utf8'))
    const offenders = Object.entries(messages).flatMap(([key, entry]) => {
      const rest = (EXCEPTIONS[locale][key] ?? []).reduce((text, phrase) => text.replace(phrase, ''), entry.message)
      return offends(rest) ? [`${key}: ${entry.message}`] : []
    })
    assert.deepEqual(offenders, [])
  })
}
