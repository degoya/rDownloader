/**
 * French says "vous", Spanish says "tú" (owner, 2026-10-02; `design.md`), the sibling of the
 * German "du" rule in `germanAddress.test.ts`.
 *
 * Both catalogues had drifted: about fifty French strings addressed the reader as "tu" and about
 * forty-five Spanish ones as "usted", mostly in features written later than their neighbours. No
 * pattern can tell every imperative from a third-person description in either language — French
 * "Vérifie les fichiers" is the plugin describing itself, Spanish "hasta que el servicio se
 * reinicie" is a subjunctive — so the check is a heuristic of the forms that only ever address
 * the reader:
 *
 * - French: the pronouns and possessives of "tu" (tu, toi, ton, ta, tes, te, tien…), an elided
 *   "t'" before a word, the "tu" imperatives of irregular verbs that are no past participle
 *   (mets, fais, prends, attends, vois, lis, écris), and an imperative with a hyphenated pronoun
 *   whose verb does not end in "z" (installe-en, connecte-toi, saisis-le — never installez-en).
 * - Spanish: "usted"/"ustedes", an accented reflexive "usted" imperative (asegúrese, diríjase),
 *   and a formal imperative from the list below at the start of a clause (Introduzca, Pulse…);
 *   the "tú" forms (Introduce, Pulsa) differ in their ending.
 *
 * A sentence the heuristic misreads goes into EXCEPTIONS by key and phrase, as in the German test;
 * the phrase is cut out of that one string only. `extension/test/french-spanish-address.test.mjs`
 * and `crates/rdownloader/tests/repo_lints/french_spanish_address.rs` hold the same rule for the extension
 * and the plugin catalogues.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const locales = resolve(dirname(fileURLToPath(import.meta.url)), '../locales')

const FRENCH_WORDS = new Set([
  'tu', 'toi', 'ton', 'ta', 'tes', 'te', 'tien', 'tienne', 'tiens', 'tiennes',
  'mets', 'fais', 'prends', 'reprends', 'attends', 'vois', 'lis', 'relis', 'écris'
])
const FRENCH_HYPHENATED = new Set(['toi', 'moi', 'en', 'le', 'la', 'les', 'y'])
/**
 * French imperatives that are also a third person ("Vérifie les fichiers" describes the plugin), and
 * the English ones they translate (RD-1480-15): such a verb at the start of a clause addresses the
 * reader when an English clause of the same string starts with the base form.
 */
const FRENCH_IMPERATIVES: Readonly<Record<string, string>> = {
  active: 'enable|turn|switch', ajoute: 'add', annule: 'cancel|undo', approuve: 'approve', appuie: 'press',
  arrête: 'stop', associe: 'pair|link', attribue: 'assign', autorise: 'allow', change: 'change',
  charge: 'load', cherche: 'search|look', choisis: 'choose|pick|select', clique: 'click|press',
  coche: 'tick|check', colle: 'paste', configure: 'set|configure', confirme: 'confirm',
  connecte: 'connect|sign', conserve: 'keep', consulte: 'see|check', corrige: 'fix|correct',
  crée: 'create|add|set', décoche: 'untick|uncheck', définis: 'set|define', délivre: 'issue',
  demande: 'ask', démarre: 'start', déplace: 'move', désactive: 'disable|turn|switch', donne: 'give',
  enregistre: 'save|store', entre: 'enter', envoie: 'send|submit', essaie: 'try', exporte: 'export',
  ferme: 'close', garde: 'keep', importe: 'import', indique: 'name|give|state|enter|specify|set|point',
  installe: 'install', laisse: 'leave|let|keep', lance: 'start|run', libère: 'free', modifie: 'change|edit',
  nomme: 'name', ouvre: 'open', pointe: 'point', réduis: 'reduce|lower|shorten', réessaie: 'try|retry',
  règle: 'set|adjust', recharge: 'reload', redémarre: 'restart', regarde: 'look|see',
  relance: 'run|restart|start|retry', remplace: 'replace', remplis: 'fill', renseigne: 'enter|set|fill',
  répète: 'repeat', résous: 'solve', restaure: 'restore', retire: 'remove|take', saisis: 'enter|type',
  sélectionne: 'choose|pick|select', signale: 'report', supprime: 'delete|remove',
  télécharge: 'download', transmets: 'hand|pass', utilise: 'use', vérifie: 'check|verify|make|see|look'
}
const CLAUSE = /[.!?:;—–(|«“]|,\s*(?:or|and|then|ou|et|puis)?(?=\s)|\s(?:or|and|then|ou|et|puis)(?=\s)/u
const SPANISH_WORDS = new Set(['usted', 'ustedes'])
const SPANISH_IMPERATIVES = new Set([
  'abra', 'acepte', 'active', 'actualice', 'añada', 'apruebe', 'asigne', 'borre', 'cambie',
  'cancele', 'cierre', 'compare', 'compruebe', 'configure', 'confirme', 'conecte', 'consulte',
  'copie', 'cree', 'defina', 'deje', 'descargue', 'desactive', 'desmarque', 'detenga', 'ejecute',
  'elija', 'elimine', 'escanee', 'escriba', 'espere', 'guarde', 'haga', 'indique', 'inicie',
  'instale', 'intente', 'introduzca', 'marque', 'mueva', 'pegue', 'permita', 'ponga', 'pulse',
  'quite', 'rechace', 'recargue', 'reinicie', 'resuelva', 'responda', 'revise', 'seleccione',
  'use', 'utilice', 'vincule', 'vuelva', 'limite', 'repita'
])

/** Phrases the heuristic misreads, as `<catalogue>:<key>` and the phrase that carries them. */
const EXCEPTIONS: Readonly<Record<'fr' | 'es', Readonly<Record<string, readonly string[]>>>> = {
  fr: {},
  es: {}
}

const words = (text: string): string[] => text.split(/[^\p{L}\p{N}'’-]+/u).filter(Boolean)

function addressesAsTu(text: string): boolean {
  return words(text).some((word) => {
    const lower = word.toLowerCase()
    if (FRENCH_WORDS.has(lower) || /^t['’]\p{L}/u.test(lower)) return true
    const parts = lower.split('-')
    const pronoun = parts.at(-1) ?? ''
    const verb = parts.at(-2) ?? ''
    return parts.length > 1 && FRENCH_HYPHENATED.has(pronoun) && verb !== '' && !verb.endsWith('z')
  })
}

const clauseStarts = (text: string): string[] =>
  text.split(CLAUSE).map(clause => /^\s*(?:ne\s+|n['’])?(\p{L}+)/u.exec(clause)?.[1]?.toLowerCase() ?? '').filter(Boolean)

/** The French imperatives of `text` that an English imperative of the same string answers for. */
function imperativesOf(text: string, english: string): string[] {
  const commands = new Set(clauseStarts(english))
  return clauseStarts(text).filter(verb => FRENCH_IMPERATIVES[verb]?.split('|').some(base => commands.has(base)))
}

function addressesAsUsted(text: string): boolean {
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

function flatten(value: unknown, prefix: string, out: Map<string, string>): void {
  if (typeof value === 'string') {
    out.set(prefix, value)
    return
  }
  if (typeof value !== 'object' || value === null) return
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    flatten(child, prefix.endsWith(':') ? `${prefix}${key}` : `${prefix}.${key}`, out)
  }
}

function catalogueStrings(locale: 'en' | 'fr' | 'es'): Map<string, string> {
  const directory = join(locales, locale)
  const out = new Map<string, string>()
  for (const name of readdirSync(directory).filter((file) => file.endsWith('.json')).sort()) {
    const catalogue: unknown = JSON.parse(readFileSync(join(directory, name), 'utf8'))
    flatten(catalogue, `${name.replace(/\.json$/, '')}:`, out)
  }
  return out
}

const RULES = [
  { locale: 'fr', name: 'French never addresses the reader as "tu"', offends: addressesAsTu },
  { locale: 'es', name: 'Spanish never addresses the reader as "usted"', offends: addressesAsUsted }
] as const

describe.each(RULES)('$locale catalogue address', ({ locale, name, offends }) => {
  const strings = catalogueStrings(locale)
  const exceptions = EXCEPTIONS[locale]

  it('reads the catalogues', () => {
    expect(strings.size).toBeGreaterThan(1000)
  })

  it(name, () => {
    const offenders = [...strings].flatMap(([key, text]) => {
      const rest = (exceptions[key] ?? []).reduce((acc, phrase) => acc.replace(phrase, ''), text)
      return offends(rest) ? [`${key}: ${text}`] : []
    })
    expect(offenders).toEqual([])
  })

  // A rewritten sentence leaves its entry behind; the list stays as short as the catalogue needs.
  it('lists only exceptions that are still there', () => {
    const stale = Object.entries(exceptions).flatMap(([key, phrases]) =>
      phrases.filter((phrase) => !(strings.get(key) ?? '').includes(phrase)).map((phrase) => `${key}: ${phrase}`)
    )
    expect(stale).toEqual([])
  })

  it('recognises the forms it is meant to find', () => {
    const samples = {
      fr: ['Installe-en un.', 'Tu as la version la plus récente.', 'Mets à jour {tool}.', "Si l'échec se répète, signale-le."],
      es: ['Introduzca la clave.', 'Asegúrese de guardar.', 'Elegido por usted', 'Hay una clave. Pulse el botón.']
    }[locale]
    const clean = {
      fr: ['Installez-en un.', 'Vous avez la version la plus récente.', 'Fichiers choisis', 'Vérifie les fichiers .md5.', 'peut-être'],
      es: ['Introduce la clave.', 'Asegúrate de guardar.', 'Elegido por ti', 'hasta que el servicio se reinicie', 'Descargándose']
    }[locale]
    expect(samples.filter((text) => !offends(text))).toEqual([])
    expect(clean.filter((text) => offends(text))).toEqual([])
  })
})

describe('French imperatives against English', () => {
  it('address the reader as "vous"', () => {
    const english = catalogueStrings('en')
    const offenders = [...catalogueStrings('fr')]
      .filter(([key]) => !key.startsWith('nav:search.keywords'))
      .flatMap(([key, text]) => imperativesOf(text, english.get(key) ?? '').map(verb => `${key}: ${verb}: ${text}`))
    expect(offenders).toEqual([])
  })

  it('recognises an imperative and leaves a description alone', () => {
    expect(imperativesOf('Occupé ; réessaie', 'Busy; try again')).toEqual(['réessaie'])
    expect(imperativesOf('Choisis un port ou ferme ce programme.', 'Choose a port or close that program.')).toEqual(['choisis', 'ferme'])
    expect(imperativesOf('Vérifie les fichiers.', 'Checks the files.')).toEqual([])
    expect(imperativesOf('Choisissez un port.', 'Choose a port.')).toEqual([])
  })
})
