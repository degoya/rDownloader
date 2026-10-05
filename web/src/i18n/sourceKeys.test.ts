/**
 * Every translation key the *code* asks for, checked against the catalogue that has to answer.
 *
 * `locales.test.ts` compares the four languages with each other, which cannot see a key that is
 * missing — or misplaced — in all four at once. Two of those got through in one sitting:
 *
 * - Six server codes were written as `proxy: { deleted: … }` *beside* `codes` instead of flat
 *   *inside* it, because `scripts/i18n-key.sh` splits a dotted key into a nested group. Nothing
 *   resolved `server.codes.proxy.deleted` in any language, and all four agreed.
 * - `plugins.type.oauth` was missing from all four catalogues, so an installed OAuth plugin
 *   showed the raw key where its type belonged.
 *
 * So this file resolves keys taken from where they are *produced* rather than from the
 * catalogues: literal `t('…')` calls in the sources, the failure and message codes the REST
 * layer constructs, and the plugin types the manifest parser knows.
 *
 * **What it does not see.** Be aware of the holes rather than trusting a green run:
 *
 * - **Composed keys.** `t(\`plugins.type.${type}\`)`, `t('common.' + name)` and anything built
 *   from a variable are invisible to a regular expression. Plugin types are covered because
 *   their *values* are extracted separately; other composed keys are not covered at all.
 * - **Codes from outside the `rd-api` crates, except `Failure::coded`.** A `Failure` raised in
 *   `rd-core`, `rd-scheduler` or a runner reaches the interface through the same `code` field.
 *   Its literal code is read from every crate, directly or through a helper whose first
 *   parameter is `code: &str` (RD-190-18); a code chosen by an enum's own `code()` method, as
 *   `rd-tools` does, is not. Plugin codes (`<slug>.<condition>`) are deliberately out of scope:
 *   they live in the package's own catalogue, which `pluginMessages.test.ts` covers.
 * - **Codes that reach the client another way.** Only the `ApiError::*` constructors and
 *   `MessageResponse::new` are read. A code passed to a validation helper as an argument, or
 *   assembled at runtime, is not seen — `proxy.password_invalid` is exactly such a case and is
 *   only covered here because its five siblings are constructed directly.
 * - **Whether a translation is *right*.** This asks whether a key resolves, never whether the
 *   sentence behind it says what the code means.
 */
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { beforeAll, describe, expect, it } from 'vitest'

import { REQUIRED_LOCALES, i18n } from '@/i18n'
import { loadEveryLocale } from '@/test/locales'

beforeAll(loadEveryLocale)

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const crates = join(repositoryRoot, 'crates')
const manifestSource = join(repositoryRoot, 'crates/rd-plugin-host/src/manifest.rs')
/** The web package is tested inside the repository; without the crates there is nothing to read. */
const backendAvailable = existsSync(join(crates, 'rd-api/src')) && existsSync(manifestSource)
/**
 * `rd-api` and the crates it was split into (RD-160-06): `rd-api-core`, `rd-api-intake`, … Read
 * from the directory rather than listed, so a further split cannot hide its codes from this file.
 */
const apiSources: string[] = backendAvailable
  ? readdirSync(crates, { withFileTypes: true })
      .filter(entry => entry.isDirectory() && /^rd-api(-[a-z]+)?$/.test(entry.name))
      .map(entry => join(crates, entry.name, 'src'))
      .filter(existsSync)
  : []

/**
 * Codes the backend constructs that no catalogue translates yet.
 *
 * Found on 2026-09-08 with 76 codes and emptied in the 1.9.1 audit (RD-191-05): every `rd-api*`
 * code now resolves in all four languages, and a new code ships its translation with it. The
 * constant stays so the exception remains explicit and reviewable rather than silent; a code
 * that gains a translation has to leave it, which `keeps the untranslated list shrinking` checks.
 */
const UNTRANSLATED_CODES: readonly string[] = []

function rustFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) return rustFiles(path)
    return entry.isFile() && entry.name.endsWith('.rs') ? [path] : []
  })
}

const CODE = '([a-z][a-z0-9_]*(?:\\.[a-z0-9_]+)+)'
/** The REST constructors that carry a stable code, and the success message that does the same. */
const CODE_CALLS = [
  new RegExp(
    'ApiError::(?:bad_request|unauthorized|forbidden|not_found|conflict|unprocessable'
      + `|too_many_requests|bad_gateway)\\s*\\(\\s*"${CODE}"`,
    'g'
  ),
  new RegExp(`MessageResponse::new\\s*\\(\\s*"${CODE}"`, 'g')
]

function backendCodes(): string[] {
  const codes = new Set<string>()
  for (const file of apiSources.flatMap(rustFiles)) {
    const source = readFileSync(file, 'utf8')
    for (const pattern of CODE_CALLS) {
      for (const match of source.matchAll(pattern)) if (match[1]) codes.add(match[1])
    }
  }
  return [...codes].sort()
}

/** Every crate's sources without its tests: a `#[cfg(test)]` module or a `tests` file raises codes nobody sees. */
function runtimeSources(): string[] {
  return readdirSync(crates, { withFileTypes: true })
    .filter(entry => entry.isDirectory())
    .map(entry => join(crates, entry.name, 'src'))
    .filter(existsSync)
    .flatMap(rustFiles)
    .filter(file => !/(?:^|[/\\])tests(?:[/\\]|\.rs$)|_tests\.rs$/.test(file))
    .map((file) => {
      const source = readFileSync(file, 'utf8')
      const tests = source.indexOf('#[cfg(test)]\nmod tests')
      return tests < 0 ? source : source.slice(0, tests)
    })
}

/**
 * The codes a runtime `Failure` is built with anywhere in the workspace (RD-190-18).
 *
 * A download, a plugin or a transfer that fails carries its code to the queue the same way a
 * REST error does, and three dozen of them reached the reader as English prose in every language.
 * Read are `Failure::coded(<kind>, "<code>", …)` and the calls of a helper that takes the code
 * as its first parameter and builds the failure itself, such as `transient("plugin.net_timeout", …)`.
 */
function runtimeFailureCodes(): string[] {
  const codes = new Set<string>()
  const coded = new RegExp(`Failure::coded\\(\\s*[^";]{0,200}?"${CODE}"`, 'g')
  const helper = /fn (\w+)\s*(?:<[^>]*>)?\(\s*code: &(?:'static )?str[^)]*\)[^{]*\{/g
  for (const source of runtimeSources()) {
    for (const match of source.matchAll(coded)) if (match[1]) codes.add(match[1])
    for (const match of source.matchAll(helper)) {
      const body = source.slice((match.index ?? 0) + match[0].length).slice(0, 600)
      if (!match[1] || !body.includes('Failure::coded(')) continue
      for (const call of source.matchAll(new RegExp(`\\b${match[1]}\\(\\s*"${CODE}"`, 'g'))) {
        if (call[1]) codes.add(call[1])
      }
    }
  }
  return [...codes].sort()
}

/** The `plugin_type` values the manifest parser accepts, read from `PluginType::as_str`. */
function pluginTypes(): string[] {
  const source = readFileSync(manifestSource, 'utf8')
  const start = source.indexOf('pub fn as_str')
  const body = source.slice(start, source.indexOf('\n    }\n', start))
  // `[a-z-]` and not `[a-z]`: the eleventh type is `remote-job`, and a character class that
  // stopped at the hyphen matched nothing for that arm — which would have made this test pass
  // by not seeing the very type it exists to check (RD-107-06).
  return [...body.matchAll(/Self::\w+\s*=>\s*"([a-z-]+)"/g)].map(match => match[1] ?? '')
}

describe.skipIf(!backendAvailable)('keys the backend produces', () => {
  it('translates every failure and message code the REST layer constructs', () => {
    i18n.global.locale.value = 'en'
    const codes = backendCodes()
    // A renamed constructor would empty the extraction and turn this into a test that proves
    // nothing. The count only has to be in the right order of magnitude to catch that.
    expect(codes.length).toBeGreaterThan(300)

    const known = new Set(UNTRANSLATED_CODES)
    const missing = codes.filter(code => !known.has(code) && !i18n.global.te(`server.codes.${code}`))
    expect(missing).toEqual([])
  })

  it('translates every code a runtime failure is built with, in any crate (RD-190-18)', () => {
    i18n.global.locale.value = 'en'
    const codes = runtimeFailureCodes()
    // A renamed constructor would empty the extraction and prove nothing; both halves count.
    expect(codes.length).toBeGreaterThan(100)
    expect(codes).toContain('plugin.net_timeout')
    expect(codes.filter(code => !i18n.global.te(`server.codes.${code}`))).toEqual([])
  })

  // A LinkGrabber candidate carries its message in the database, not in a REST body, so none
  // of the constructors above sees it. It used to be English prose printed verbatim — one
  // sentence, "Check result missing", for three unrelated situations (RD-109-43). These codes
  // have to resolve in every language from the day they are written, not land on the list
  // above.
  it.each(REQUIRED_LOCALES)('%s translates every candidate check code', (locale) => {
    i18n.global.locale.value = locale as 'en'
    const codes = new Set<string>()
    for (const file of apiSources.flatMap(rustFiles)) {
      const source = readFileSync(file, 'utf8')
      for (const match of source.matchAll(
        new RegExp(`CandidateMessage::coded\\(\\s*"${CODE}"`, 'g')
      )) {
        if (match[1]) codes.add(match[1])
      }
    }
    // A renamed constructor would empty the extraction and prove nothing.
    expect(codes.size).toBeGreaterThanOrEqual(11)
    const missing = [...codes].filter(code => !i18n.global.te(`server.codes.${code}`))
    expect(missing).toEqual([])
  })

  it('keeps the untranslated list shrinking', () => {
    i18n.global.locale.value = 'en'
    const translated = UNTRANSLATED_CODES.filter(code => i18n.global.te(`server.codes.${code}`))
    expect(translated).toEqual([])
  })

  it('names every plugin type the manifest parser accepts', () => {
    i18n.global.locale.value = 'en'
    const types = pluginTypes()
    expect(types).toContain('oauth')
    expect(types).toContain('remote-job')
    expect(types.length).toBeGreaterThanOrEqual(11)
    expect(types.filter(type => !i18n.global.te(`plugins.type.${type}`))).toEqual([])
  })
})

describe('the shape a server catalogue has to keep', () => {
  // `scripts/i18n-key.sh` turns a dotted key into a nested group, which is right for
  // `plugins.json` and wrong for `server.json`: a code is one literal key inside `codes`.
  // Anything else resolves nowhere, in every language at once.
  it.each(REQUIRED_LOCALES)('%s keeps every code flat inside `codes`', (locale) => {
    const catalogue = i18n.global.getLocaleMessage(locale) as unknown as {
      server: Record<string, unknown>
    }
    expect(Object.keys(catalogue.server).sort()).toEqual(['capabilities', 'codes'])
    const nested = Object.entries(catalogue.server.codes as Record<string, unknown>)
      .filter(([, value]) => typeof value !== 'string')
      .map(([key]) => key)
    expect(nested).toEqual([])
  })
})

describe('keys the sources ask for', () => {
  // `locales.test.ts` resolves the literal keys in views and components. Stores, composables
  // and helpers translate too — a toast built in a store is as visible as a button label.
  const sources = {
    ...import.meta.glob('@/stores/**/*.ts', { eager: true, query: '?raw', import: 'default' }),
    ...import.meta.glob('@/composables/**/*.ts', { eager: true, query: '?raw', import: 'default' }),
    ...import.meta.glob('@/utils/**/*.ts', { eager: true, query: '?raw', import: 'default' })
  }

  /** Literal `t('…')` / `$t("…")` calls. A key built from a variable is not one of these. */
  function literalKeys(source: string): string[] {
    return [...source.matchAll(/\$?\bt\(\s*'([a-z][\w.]*)'/g)].map(match => match[1] ?? '')
      .concat([...source.matchAll(/\$?\bt\(\s*"([a-z][\w.]*)"/g)].map(match => match[1] ?? ''))
  }

  it('resolves every literal translation key used outside a template', () => {
    i18n.global.locale.value = 'en'
    const missing: string[] = []
    for (const [path, source] of Object.entries(sources)) {
      for (const key of literalKeys(source as string)) {
        if (!i18n.global.te(key)) missing.push(`${path}: ${key}`)
      }
    }
    expect(missing).toEqual([])
  })
})
