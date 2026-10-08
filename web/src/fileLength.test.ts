// @vitest-environment node
/**
 * At most 500 lines per file (owner, 2026-10-08, DOC-13; RD-1190-07), held by a ratchet for the
 * web interface and the browser extension.
 *
 * A production file over the limit is split — a module of its own, re-exported from the old one so
 * every importer keeps its path. A test file over the limit on 2026-10-08 is in `BASELINE` with its
 * count then: it may shrink and never grow past that count, and an entry whose file is back at the
 * limit, or gone, leaves the list. A test file not on the list stays at the limit.
 *
 * Lines are counted as `wc -l` counts them. Generated files are not ours to split. The Rust
 * sources have their own lint (`crates/rdownloader/tests/repo_lints/file_length.rs`), the scripts
 * theirs (`scripts/tests/file-length.sh`).
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const LIMIT = 500
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const TOPS = ['web', 'extension']
const SKIPPED_DIRECTORIES = new Set(['node_modules', 'dist'])
const SOURCE = /\.(?:ts|vue|js|mjs|cjs|css|html)$/

/** Written by a generator (AGENTS.md, Conventions), never by hand. */
const GENERATED = new Set(['web/src/api/schema.d.ts', 'web/components.d.ts', 'web/auto-imports.d.ts'])

/** Test files above the limit on 2026-10-08 and their line counts then; may only go down. */
const BASELINE: Record<string, number> = {
  'extension/test/captcha.test.mjs': 752,
  'web/src/components/CollectorCandidateRow.test.ts': 602,
  'web/src/components/IndexerReviewList.test.ts': 672,
  'web/src/components/IndexerSearchPanel.test.ts': 551,
  'web/src/components/routing/RoutingCategories.test.ts': 547,
  'web/src/components/settings/SettingsAccountsCard.test.ts': 591,
  'web/src/components/settings/SettingsPluginsTab.test.ts': 995,
  'web/src/views/DownloadsView.test.ts': 920,
  'web/src/views/LinkGrabberView.test.ts': 1281,
  'web/src/views/SubscriptionsView.test.ts': 1011
}

/** Every source below `directory`, relative to the repository root with `/`. */
function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      return entry.name.startsWith('.') || SKIPPED_DIRECTORIES.has(entry.name) ? [] : sources(path)
    }
    return entry.isFile() && SOURCE.test(entry.name) ? [relative(repoRoot, path).split('\\').join('/')] : []
  })
}

function isTest(path: string): boolean {
  return /\.test\.[cm]?[jt]s$/.test(path) || path.split('/').some(part => part === 'test' || part === 'e2e')
}

/** What is wrong with the files over the limit, given as `[path, lines]`; empty when nothing is. */
function findings(over: [string, number][], baseline: Record<string, number>): string[] {
  const found: string[] = []
  for (const [path, lines] of over) {
    const counted = baseline[path]
    if (counted === undefined) {
      found.push(`${path}: ${lines} lines${isTest(path) ? '' : ' of production code'}; split it`)
    } else if (lines > counted) {
      found.push(`${path}: ${lines} lines, grown past its ${counted}; split it`)
    }
  }
  for (const path of Object.keys(baseline)) {
    if (!over.some(([file]) => file === path)) {
      found.push(`${path}: at or under ${LIMIT} lines, or gone; remove it from BASELINE`)
    } else if (!isTest(path)) {
      found.push(`${path}: production code on BASELINE; split it`)
    }
  }
  return found
}

describe('at most 500 lines per file', () => {
  it('holds the web interface and the extension to the limit, the baseline only shrinking', () => {
    const over = TOPS.flatMap(top => sources(join(repoRoot, top)))
      .filter(path => !GENERATED.has(path))
      .map((path): [string, number] => [path, readFileSync(join(repoRoot, path), 'utf8').split('\n').length - 1])
      .filter(([, lines]) => lines > LIMIT)
    expect(over.length).toBeGreaterThan(0)
    expect(findings(over, BASELINE)).toEqual([])
  })

  describe('the guard itself', () => {
    it('turns red on a new file over the limit and on a listed one that grew', () => {
      expect(findings([['web/src/a.ts', 501]], {})).toEqual(['web/src/a.ts: 501 lines of production code; split it'])
      expect(findings([['web/src/a.test.ts', 610]], { 'web/src/a.test.ts': 600 }))
        .toEqual(['web/src/a.test.ts: 610 lines, grown past its 600; split it'])
    })

    it('lets a listed test file shrink, and asks for the entry once it is back at the limit', () => {
      expect(findings([['web/src/a.test.ts', 590]], { 'web/src/a.test.ts': 600 })).toEqual([])
      expect(findings([], { 'web/src/a.test.ts': 600 }))
        .toEqual(['web/src/a.test.ts: at or under 500 lines, or gone; remove it from BASELINE'])
    })

    it('keeps production code off the baseline', () => {
      expect(findings([['web/src/a.ts', 520]], { 'web/src/a.ts': 520 }))
        .toEqual(['web/src/a.ts: production code on BASELINE; split it'])
    })
  })
})
