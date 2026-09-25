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
 * - **Codes from outside `crates/rd-api/src`.** A `Failure` raised in `rd-core`, `rd-scheduler`
 *   or a runner reaches the interface through the same `code` field, and nothing here extracts
 *   those. Plugin codes (`<slug>.<condition>`) are deliberately out of scope: they live in the
 *   package's own catalogue, which `pluginMessages.test.ts` covers.
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

import { describe, expect, it } from 'vitest'

import { i18n } from '@/i18n'

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const apiSources = join(repositoryRoot, 'crates/rd-api/src')
const manifestSource = join(repositoryRoot, 'crates/rd-plugin-host/src/manifest.rs')
/** The web package is tested inside the repository; without the crates there is nothing to read. */
const backendAvailable = existsSync(apiSources) && existsSync(manifestSource)

/**
 * Codes the backend constructs that no catalogue translates yet, as found on 2026-09-08.
 *
 * They are listed rather than fixed because 76 codes are 304 sentences, which belongs in a
 * translation pass and not in the change that first made them visible. Every one of them
 * currently reaches the reader as the backend's English prose in all four languages. The list
 * may only shrink: a code that gains a translation has to leave it, and the second test below
 * fails while it lingers.
 */
const UNTRANSLATED_CODES: readonly string[] = [
  'account.credential_mode_required',
  'account.credential_mode_unsupported',
  'api.token_not_found',
  'api.token_required',
  'api.token_revoked',
  'automation.deleted',
  'automation.dry_run_target_missing',
  'automation.not_found',
  'backup.area_missing',
  'backup.format_unsupported',
  'backup.version_unsupported',
  'capture.field_length',
  'capture.header_length',
  'capture.headers_limit',
  'capture.links_limit',
  'capture.method_unsupported',
  'collector.auth_profile_disabled',
  'collector.auth_profile_missing',
  'collector.auth_profile_not_found',
  'collector.auth_profile_scope_mismatch',
  'collector.candidate_missing',
  'collector.check_target_missing',
  'collector.enqueue_failed',
  'collector.no_links',
  'collector.nzb_client_unavailable',
  'collector.nzb_empty',
  'collector.nzb_fetch_failed',
  'collector.nzb_invalid',
  'collector.nzb_rejected',
  'collector.package_empty',
  'nzb.no_change',
  'plugin.disabled',
  'plugin.enabled',
  'plugin.not_installed',
  'plugin.remove_failed',
  'plugin.removed',
  'reconnect.already_running',
  'reconnect.disabled',
  'reconnect.script_missing',
  'reconnect.started',
  'remote.parallel_invalid',
  'remote.timeout_invalid',
  'request.multipart_missing_file',
  'settings.auto_remove_delay_invalid',
  'settings.managed_tools_manifest_url_invalid',
  'settings.patch_empty',
  'settings.proxy_invalid',
  'settings.reconnect_interval_invalid',
  'settings.reconnect_script_missing',
  'settings.reconnect_timeout_invalid',
  'settings.reconnect_url_invalid',
  'settings.tool_compatibility_override_invalid',
  'settings.unknown_key',
  'stream.recording_policy_invalid',
  'stream.schedule_name_invalid',
  'stream.schedule_not_found',
  'subscription.api_key_missing',
  'subscription.caps_failed',
  'subscription.caps_invalid',
  'subscription.category_map_too_many',
  'subscription.client_unavailable',
  'subscription.duration_range_invalid',
  'subscription.filters_too_many',
  'subscription.interval_invalid',
  'subscription.item_not_found',
  'subscription.name_invalid',
  'subscription.not_an_indexer',
  'subscription.not_found',
  'subscription.source_categories_too_many',
  'subscription.url_invalid',
  'subscription.url_scheme',
  'tools.version_not_installed',
  'torrent.priority_invalid'
]

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
  for (const file of rustFiles(apiSources)) {
    const source = readFileSync(file, 'utf8')
    for (const pattern of CODE_CALLS) {
      for (const match of source.matchAll(pattern)) if (match[1]) codes.add(match[1])
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

  // A LinkGrabber candidate carries its message in the database, not in a REST body, so none
  // of the constructors above sees it. It used to be English prose printed verbatim — one
  // sentence, "Check result missing", for three unrelated situations (RD-109-43). These codes
  // have to resolve in every language from the day they are written, not land on the list
  // above.
  it.each(['en', 'de', 'fr', 'es'])('%s translates every candidate check code', (locale) => {
    i18n.global.locale.value = locale as 'en'
    const codes = new Set<string>()
    for (const file of rustFiles(apiSources)) {
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
  it.each(['en', 'de', 'fr', 'es'])('%s keeps every code flat inside `codes`', (locale) => {
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
