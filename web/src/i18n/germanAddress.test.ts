/**
 * The German interface says "du", never "Sie" (owner, 2026-09-27, RD-150-16; `design.md`).
 *
 * The catalogue had drifted into both: about ninety sentences addressed the reader formally and
 * twenty informally, depending on who wrote the feature. A formal address is always capitalised
 * — "Sie", "Ihnen", "Ihr…" — so any capitalised form is a finding. The one legitimate capital is
 * the third person at the start of a sentence ("Sie laufen in dieser Reihenfolge" — the steps),
 * which no pattern can tell from "Sie brauchen eine sichere Verbindung" (the reader). Those are
 * listed below by key and phrase; the phrase is cut out of that one string only, so a formal
 * address added elsewhere in the same sentence is still found.
 *
 * `extension/test/german-address.test.mjs` and `crates/rdownloader/tests/repo_lints/german_address.rs`
 * hold the same rule for the extension and the plugin catalogues.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const catalogues = resolve(dirname(fileURLToPath(import.meta.url)), '../locales/de')

const FORMAL = /(?<!\p{L})(Sie|Ihnen|Ihr(?:e[mnrs]?)?)(?!\p{L})/u

/** Third person at a sentence start, as `<catalogue>:<key>` and the phrase that carries it. */
const THIRD_PERSON: Readonly<Record<string, readonly string[]>> = {
  'captcha:settings.hint': ['Sie werden entweder'],
  'captcha:widget.extension_connected': ['Sie meldet'],
  'linkgrabber:candidate.duplicate_hint': ['Sie kann'],
  'linkgrabber:candidate.unresolvable_hint': ['Sie lässt'],
  'linkgrabber:confirm.dissolve_mirror_description': ['Sie werden auch'],
  'linkgrabber:intake.skipped_disabled': ['Ihr Dienst'],
  'plugins:card.superseded_hint': ['Sie bleibt'],
  'plugins:incompatible.description': ['Sie bleiben'],
  'reconnect:script_description': ['Sie läuft'],
  'routing:category.delete_description': ['Ihre Regeln'],
  'settings:auth_profiles.approval_description': ['Sie werden erst'],
  'settings:postprocess.plugin_steps.description': ['Sie laufen'],
  'siterules:steps.description': ['Sie laufen'],
  'torrent:patterns.description': ['Sie wirken']
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

function germanStrings(): Map<string, string> {
  const out = new Map<string, string>()
  for (const name of readdirSync(catalogues).filter((file) => file.endsWith('.json')).sort()) {
    const catalogue: unknown = JSON.parse(readFileSync(join(catalogues, name), 'utf8'))
    flatten(catalogue, `${name.replace(/\.json$/, '')}:`, out)
  }
  return out
}

describe('German catalogue address', () => {
  const strings = germanStrings()

  it('reads the catalogues', () => {
    expect(strings.size).toBeGreaterThan(1000)
  })

  it('never addresses the reader as "Sie"', () => {
    const offenders = [...strings].flatMap(([key, text]) => {
      const rest = (THIRD_PERSON[key] ?? []).reduce((acc, phrase) => acc.replace(phrase, ''), text)
      return FORMAL.test(rest) ? [`${key}: ${text}`] : []
    })
    expect(offenders).toEqual([])
  })

  // A rewritten sentence leaves its entry behind; the list stays as short as the catalogue needs.
  it('lists only third-person phrases that are still there', () => {
    const stale = Object.entries(THIRD_PERSON).flatMap(([key, phrases]) =>
      phrases.filter((phrase) => !(strings.get(key) ?? '').includes(phrase)).map((phrase) => `${key}: ${phrase}`)
    )
    expect(stale).toEqual([])
  })
})
