import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '@/i18n'
import {
  loadPluginMessages,
  providerText,
  resetPluginMessages,
  setPluginMessagesAvailable
} from '@/i18n/plugins'
import { translateAccountLabel, translateServerMessage } from '@/i18n/server'

/** Response bodies keyed by locale, as `/api/v1/plugins/i18n/{locale}` would return them. */
const BUNDLES: Record<string, unknown> = {
  en: {
    server: { codes: { 'fastshare.bad_credentials': 'FastShare login failed' } },
    providers: {
      fastshare: {
        name: 'FastShare',
        description: 'Resolves FastShare links',
        secret_label: 'API key',
        secret_hint: 'Found in your FastShare account'
      }
    }
  },
  de: {
    server: { codes: { 'fastshare.bad_credentials': 'FastShare-Anmeldung fehlgeschlagen' } },
    providers: { fastshare: { name: 'FastShare', description: 'Loest FastShare-Links auf' } }
  }
}

function mockFetch(bundles: Record<string, unknown> = BUNDLES): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string) => {
      const locale = input.split('/').pop() ?? ''
      const body = bundles[locale]
      return body === undefined
        ? ({ ok: false, status: 404, json: async () => ({}) } as Response)
        : ({ ok: true, status: 200, json: async () => body } as Response)
    })
  )
}

describe('plugin-supplied translations', () => {
  const LOCALES = ['en', 'de'] as const
  // Merging mutates the shared i18n instance. vue-i18n compiles messages into functions, so
  // they cannot be cloned wholesale; instead each test records what the core catalogue holds
  // in the two subtrees a plugin bundle touches and rolls those back afterwards.
  let coreCodeKeys: Record<string, string[]> = {}
  let originalLocale: string

  function messagesOf(locale: string): Record<string, Record<string, unknown>> {
    return i18n.global.getLocaleMessage(locale as never) as unknown as Record<
      string,
      Record<string, unknown>
    >
  }

  beforeEach(() => {
    // These translations live behind the sign-in, so every case below stands in for a signed-in
    // session; the one that does not says so itself.
    setPluginMessagesAvailable(true)
    resetPluginMessages()
    originalLocale = i18n.global.locale.value
    coreCodeKeys = Object.fromEntries(
      LOCALES.map(locale => [locale, Object.keys(messagesOf(locale).server?.codes ?? {})])
    )
    i18n.global.locale.value = 'en'
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    resetPluginMessages()
    for (const locale of LOCALES) {
      const messages = messagesOf(locale)
      const codes = messages.server?.codes as Record<string, unknown> | undefined
      if (codes) {
        const core = new Set(coreCodeKeys[locale])
        for (const code of Object.keys(codes)) if (!core.has(code)) delete codes[code]
      }
      // `providers` only ever comes from a plugin bundle.
      delete messages.providers
    }
    i18n.global.locale.value = originalLocale as never
  })

  it('merges a plugin failure code into the server catalogue', async () => {
    mockFetch()
    expect(translateServerMessage({ code: 'fastshare.bad_credentials', message: 'Login failed' }))
      .toBe('Login failed')

    await loadPluginMessages('en')

    expect(translateServerMessage({ code: 'fastshare.bad_credentials', message: 'Login failed' }))
      .toBe('FastShare login failed')
  })

  it('exposes localised provider labels', async () => {
    mockFetch()
    await loadPluginMessages('en')

    expect(providerText('fastshare', 'name')).toBe('FastShare')
    expect(providerText('fastshare', 'secret_label')).toBe('API key')
    expect(providerText('fastshare', 'secret_hint')).toBe('Found in your FastShare account')
    expect(providerText('fastshare', 'unknown_field')).toBeUndefined()
    expect(providerText('not_installed', 'name')).toBeUndefined()
  })

  it('always loads English alongside the active locale as the fallback layer', async () => {
    mockFetch()
    i18n.global.locale.value = 'de'
    await loadPluginMessages('de')

    const requested = (fetch as unknown as { mock: { calls: string[][] } }).mock.calls.map(call => call[0])
    expect(requested).toContain('/api/v1/plugins/i18n/de')
    expect(requested).toContain('/api/v1/plugins/i18n/en')
    expect(providerText('fastshare', 'name')).toBe('FastShare')
  })

  it("falls back to a plugin's English label when it ships no translation for the UI language", async () => {
    // The plugin translates only `en`; a German UI must still get its labels.
    mockFetch({ en: BUNDLES.en, de: { server: { codes: {} }, providers: {} } })
    i18n.global.locale.value = 'de'
    await loadPluginMessages('de')

    expect(providerText('fastshare', 'secret_label')).toBe('API key')
  })

  it('falls back to the English text the server sent when nothing translates the code', async () => {
    mockFetch({ en: { server: { codes: {} }, providers: {} } })
    await loadPluginMessages('en')

    expect(translateServerMessage({ code: 'unknown.code', message: 'Raw backend text' }))
      .toBe('Raw backend text')
  })

  it("translates a failure code through the plugin's English catalogue before the backend text", async () => {
    // The plugin ships only `en.json`; the documented chain is active language, then the
    // plugin's English, then the backend text -- the middle rung must exist for a German UI.
    mockFetch({ en: BUNDLES.en, de: { server: { codes: {} }, providers: {} } })
    i18n.global.locale.value = 'de'
    await loadPluginMessages('de')

    expect(translateServerMessage({ code: 'fastshare.bad_credentials', message: 'Raw backend text' }))
      .toBe('FastShare login failed')
  })

  it('survives an unreachable backend without throwing', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('offline') }))

    await expect(loadPluginMessages('en')).resolves.toBeUndefined()
    expect(providerText('fastshare', 'name')).toBeUndefined()
  })

  it('asks for nothing while nobody is signed in', async () => {
    // The endpoint requires a session and is deliberately not public, so asking before the
    // sign-in only ever earned a 401 in the console.
    mockFetch()
    setPluginMessagesAvailable(false)

    await loadPluginMessages('en')

    expect(fetch).not.toHaveBeenCalled()
  })

  it('loads the translations a signed-out start could not fetch, without a reload', async () => {
    // The reported sequence: the boot attempt is refused, the user signs in, and the plugin
    // strings have to arrive for that same session rather than after a page reload.
    setPluginMessagesAvailable(false)
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 401, json: async () => ({}) }) as Response))
    i18n.global.locale.value = 'de'
    await loadPluginMessages('de')
    expect(fetch).not.toHaveBeenCalled()
    expect(providerText('fastshare', 'name')).toBeUndefined()

    // Signing in is what the session store reports here.
    mockFetch()
    setPluginMessagesAvailable(true)
    await loadPluginMessages('de')

    expect(providerText('fastshare', 'name')).toBe('FastShare')
    expect(translateServerMessage({ code: 'fastshare.bad_credentials', message: 'Raw backend text' }))
      .toBe('FastShare-Anmeldung fehlgeschlagen')
  })

  it('keeps a refused locale retryable instead of remembering it as done', async () => {
    // A failure must not make every language switch ask again, but it must not become
    // permanent either: that is what left plugin strings untranslated for a whole session.
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 503, json: async () => ({}) }) as Response))
    await loadPluginMessages('en')
    await loadPluginMessages('en')
    expect(fetch).toHaveBeenCalledTimes(1)

    mockFetch()
    resetPluginMessages()
    await loadPluginMessages('en')
    expect(providerText('fastshare', 'name')).toBe('FastShare')
  })

  it('fetches each locale only once until a new plugin is installed', async () => {
    mockFetch()
    await loadPluginMessages('en')
    await loadPluginMessages('en')
    expect(fetch).toHaveBeenCalledTimes(1)

    resetPluginMessages()
    await loadPluginMessages('en')
    expect(fetch).toHaveBeenCalledTimes(2)
  })
})

describe('account label parts', () => {
  // The label next to a provider account is a list of `{ code, params, message }` parts, the
  // shape a failure already travels in, translated part by part and joined with ` · `.
  const until = { code: 'plugin.account.premium_until', params: { until: '2027-01-01' }, message: 'Premium until 2027-01-01' }
  let originalLocale: string

  beforeEach(() => {
    setPluginMessagesAvailable(true)
    resetPluginMessages()
    originalLocale = i18n.global.locale.value
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    resetPluginMessages()
    i18n.global.locale.value = originalLocale as never
    const messages = i18n.global.getLocaleMessage('de' as never) as unknown as Record<string, Record<string, unknown>>
    const codes = messages.server?.codes as Record<string, unknown> | undefined
    if (codes) delete codes['fastshare.account.tier']
    const english = i18n.global.getLocaleMessage('en') as unknown as Record<string, Record<string, unknown>>
    const englishCodes = english.server?.codes as Record<string, unknown> | undefined
    if (englishCodes) delete englishCodes['fastshare.account.tier']
  })

  it('renders a shared label part in all four languages from the core catalogue', () => {
    const expected: Record<string, string> = {
      de: 'Premium bis 2027-01-01',
      en: 'Premium until 2027-01-01',
      es: 'Premium hasta el 2027-01-01',
      fr: "Premium jusqu'au 2027-01-01"
    }
    for (const [locale, text] of Object.entries(expected)) {
      i18n.global.locale.value = locale as never
      expect(translateAccountLabel([until]), locale).toBe(text)
    }
  })

  it('pluralises the cookie count through the catalogue', () => {
    const cookies = (count: number) => ({ code: 'plugin.account.cookies', params: { count: String(count) }, message: '' })
    i18n.global.locale.value = 'en'
    expect(translateAccountLabel([cookies(0)])).toBe('no cookies loaded – downloads need a cookie session')
    expect(translateAccountLabel([cookies(1)])).toBe('1 cookie loaded')
    expect(translateAccountLabel([cookies(3)])).toBe('3 cookies loaded')
    i18n.global.locale.value = 'de'
    expect(translateAccountLabel([cookies(3)])).toBe('3 Cookies geladen')
  })

  it('joins the parts in the order the plugin sent them', () => {
    i18n.global.locale.value = 'en'
    const user = { code: 'plugin.account.user', params: { user: 'alice' }, message: 'Signed in as alice' }
    const unchecked = { code: 'plugin.account.premium_unchecked', params: {}, message: 'the subscription was not checked' }
    expect(translateAccountLabel([user, until, unchecked]))
      .toBe('Signed in as alice · Premium until 2027-01-01 · the subscription was not checked')
    expect(translateAccountLabel([])).toBe('')
    expect(translateAccountLabel(undefined)).toBe('')
  })

  it("falls back from the active language to the plugin's English catalogue, then to the backend text", async () => {
    // A provider-specific code the plugin translates only in English, shown in a German UI.
    mockFetch({
      en: { server: { codes: { 'fastshare.account.tier': 'Tier {tier}' } }, providers: {} },
      de: { server: { codes: {} }, providers: {} }
    })
    i18n.global.locale.value = 'de'
    await loadPluginMessages('de')
    const tier = { code: 'fastshare.account.tier', params: { tier: 'gold' }, message: 'Tier gold (backend)' }
    expect(translateAccountLabel([tier])).toBe('Tier gold')

    // No catalogue at all knows the code: the English text the backend sent.
    const unknown = { code: 'fastshare.account.unknown', params: {}, message: 'Backend text' }
    expect(translateAccountLabel([unknown])).toBe('Backend text')
  })

  it('shows the code itself rather than nothing when no catalogue and no text remain', () => {
    i18n.global.locale.value = 'en'
    expect(translateAccountLabel([{ code: 'fastshare.account.unknown', params: {}, message: '' }]))
      .toBe('fastshare.account.unknown')
  })
})
