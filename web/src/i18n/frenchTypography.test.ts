/**
 * French typography (RD-1480-15, `design.md`): a narrow no-break space (U+202F) before `;`, `!`
 * and `?`, a no-break space (U+00A0) before `:` and inside `« »`, the typographic apostrophe.
 *
 * With an ordinary space the punctuation mark wraps onto a line of its own; nearly every one of
 * the catalogue's thousand marks had one, and apostrophes came in both kinds. `i18n-key.sh`
 * writes a French value this way; a string that came another way is put right by
 * `python3 scripts/lib/fr_typography.py`.
 */
import { describe, expect, it } from 'vitest'

import { catalogueStrings } from '@/test/catalogues'

/** Placeholders, literals, code spans and addresses — and the same literals as the script. */
const PROTECTED = /\{'[^']*'\}|\{\w+\}|`[^`]*`|\w+:\/\/[^\s)»]+|magnet:\S+|all: true|Contents: read|SHA256:|Cookie: nom=valeur;/gu

const END = '(?=\\s|$|[)\\]»”|])'
const RULES: readonly (readonly [string, RegExp])[] = [
  ['; ! ? after U+202F', new RegExp(`[^\\u202f;!?][;!?]+${END}`, 'u')],
  [': after U+00A0', new RegExp(`[^\\u00a0:]:${END}`, 'u')],
  ['« and » with U+00A0 inside', /«(?!\u00a0)|(?<!\u00a0)»/u],
  ['the apostrophe ’', /\p{L}'(?=[\p{L}\p{N}«X])/u]
]

const faults = (text: string): string[] =>
  RULES.filter(([, rule]) => rule.test(text.replace(PROTECTED, 'X'))).map(([name]) => name)

describe('French catalogue typography', () => {
  it('holds in every string', () => {
    const strings = catalogueStrings('fr')
    expect(strings.size).toBeGreaterThan(1000)
    const found = [...strings].flatMap(([key, text]) => faults(text).map(fault => `${key}: ${fault}: ${text}`))
    expect(found, 'run python3 scripts/lib/fr_typography.py').toEqual([])
  })

  it('finds the faults and leaves formats alone', () => {
    for (const text of ['Échec : {reason}', 'Échec: {reason}', 'Fini ; suite', 'Vraiment ?', '« oui »', "l'agent", "d'{name}"]) {
      expect(faults(text), text).not.toEqual([])
    }
    for (const text of ['Échec\u00a0: {reason}', 'Fini\u202f; suite', '«\u00a0oui\u00a0»', 'l’agent', 'à 12:30', 'http://a.example/b', '!mot exclut', "{'{{'}motdepasse{'}}'}"]) {
      expect(faults(text), text).toEqual([])
    }
  })
})
