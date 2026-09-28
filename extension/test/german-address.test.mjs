// RD-150-16: the German catalogue says "du", never "Sie" (owner, 2026-09-27; `design.md`).
//
// A formal address is always capitalised, so any "Sie", "Ihnen" or "Ihr…" is a finding. The web
// catalogues carry a short list of third-person sentence starts ("Sie laufen …"); this one needs
// none, and a sentence that does gets its phrase listed here the same way
// (`web/src/i18n/germanAddress.test.ts`).

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const catalogue = join(dirname(fileURLToPath(import.meta.url)), '..', '_locales', 'de', 'messages.json')

const FORMAL = /(?<!\p{L})(Sie|Ihnen|Ihr(?:e[mnrs]?)?)(?!\p{L})/u

/** Third person at a sentence start, by message key and the phrase that carries it. */
const THIRD_PERSON = {}

test('the German extension texts never address the reader as "Sie"', () => {
  const messages = JSON.parse(readFileSync(catalogue, 'utf8'))
  const offenders = Object.entries(messages).flatMap(([key, entry]) => {
    const rest = (THIRD_PERSON[key] ?? []).reduce((text, phrase) => text.replace(phrase, ''), entry.message)
    return FORMAL.test(rest) ? [`${key}: ${entry.message}`] : []
  })
  assert.deepEqual(offenders, [])
})
