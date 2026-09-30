// @vitest-environment node
/**
 * Every download kind the API can report has a runner label in the storage history.
 *
 * `StorageActivityCard` looks the label up as `settings.storage.activity.runner.<kind>`; a kind
 * without one showed the raw key instead (`object_storage`, reported 2026-09-30). The kinds are
 * read from the generated API types, so a kind added in the backend fails here until it has a
 * label in every language.
 */
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import de from '../../locales/de/settings.json'
import en from '../../locales/en/settings.json'
import es from '../../locales/es/settings.json'
import fr from '../../locales/fr/settings.json'

const sourceRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

function downloadKinds(): string[] {
  const schema = readFileSync(join(sourceRoot, 'api', 'schema.d.ts'), 'utf8')
  const match = /DownloadKind:\s*([^;]+);/.exec(schema)
  if (!match?.[1]) throw new Error('DownloadKind not found in the generated API types')
  return [...match[1].matchAll(/"([^"]+)"/g)].map(found => found[1] ?? '')
}

describe('storage history runner labels', () => {
  const kinds = downloadKinds()

  it('reads the download kinds from the API types', () => {
    expect(kinds).toContain('http')
    expect(kinds).toContain('object_storage')
  })

  for (const [locale, catalogue] of Object.entries({ de, en, es, fr })) {
    it(`labels every download kind in ${locale}`, () => {
      const labels = catalogue.storage.activity.runner as Record<string, string>
      const missing = kinds.filter(kind => !labels[kind])
      expect(missing).toEqual([])
    })
  }
})
