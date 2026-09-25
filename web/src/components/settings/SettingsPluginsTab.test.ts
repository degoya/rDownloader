/**
 * The reported case (RD-107-17).
 *
 * "As the second screenshot shows, the plugin groups in the tabs can no longer be made out."
 * Twelve `UTabs` entries shared one line, so ten of them rendered truncated — "Benachrich… 3",
 * "Ordner-Cr… 6", "Anbietera… 1" — and the bar named nothing. That was growth rather than a
 * defect: a tab bar gives every entry a fraction of one line, so the eleventh plugin world made
 * all twelve unreadable, and a twelfth would only have made it worse.
 *
 * These tests hold the replacement to the three things the control has to do: name every group
 * in full, keep the per-group counts — the reason it is worth having at all — and survive a
 * rebuild of the view without losing the selection. The last test installs an invented twelfth
 * type and checks that nothing changes except the row's length, which is the whole point of
 * choosing a shape that wraps.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import commonCatalogue from '@/locales/en/common.json'
import pluginsCatalogue from '@/locales/en/plugins.json'
import serverCatalogue from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const remove = vi.fn()
/** The translated text `@/api/client` would derive from a coded error body. */
const responseError = vi.fn<(response: unknown) => string>()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    PATCH: vi.fn(async () => ({ data: {} })),
    DELETE: (...args: unknown[]) => remove(...args)
  },
  responseError: (response: unknown) => responseError(response),
  resultMessage: vi.fn(() => '')
}))
/** The shared confirmation dialog, so a test can read what it was asked to confirm. */
const confirmed = vi.fn<(options: Record<string, unknown>) => Promise<boolean>>()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))
// `formatMoment` reaches for the application's own i18n instance rather than the mounted one,
// so the withdrawal dates a row prints need a `d()` here; without it the mock throws mid-render.
vi.mock('@/i18n', () => ({
  currentLocale: () => 'en',
  i18n: { global: { d: (value: Date) => value.toISOString(), t: (key: string) => key } }
}))
vi.mock('@/i18n/plugins', () => ({
  providerText: () => undefined,
  loadPluginMessages: vi.fn(async () => {}),
  resetPluginMessages: vi.fn()
}))
/**
 * The shared event stream, reduced to the two handlers this tab registers.
 *
 * jsdom has no `EventSource`, and the point of the mock is anyway to be able to deliver the
 * events by hand rather than to stand up a server. There are two because the service splits
 * them by the scope their payload needs: `plugin.changed` carries the administration writes,
 * `plugin_trust.changed` the trust-store ones, whose payloads name key ids and digests.
 */
let pluginEvent: ((event: MessageEvent<string>) => void) | null = null
let trustEvent: ((event: MessageEvent<string>) => void) | null = null
/**
 * Every channel the screen subscribes to. The name matters as much as the reaction:
 * `Granted::may_observe` hands a subscriber an event only when it holds that event's
 * exact scope, so a screen listening on a channel named for another scope is silently
 * never served.
 */
let subscribedNames: string[] = []
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscribedNames = Object.keys(handlers)
    pluginEvent = handlers['plugin.changed'] ?? null
    trustEvent = handlers['plugin_trust.changed'] ?? null
    return () => { pluginEvent = null; trustEvent = null }
  }
}))
const serverMessage = vi.fn<(payload: unknown) => { code: string } | null>()
const translated = vi.fn<(message: { code: string }) => string>()
vi.mock('@/i18n/server', () => ({
  serverMessageFrom: (payload: unknown) => serverMessage(payload),
  translateServerMessage: (message: { code: string }) => translated(message)
}))

const { default: SettingsPluginsTab } = await import('./SettingsPluginsTab.vue')

/** The eleven worlds an installation can hold today; `remote-job` was the most recent addition. */
const INSTALLED_TYPES = [
  'auth',
  'crawler',
  'enricher',
  'intake',
  'notifier',
  'oauth',
  'postprocess',
  'remote-job',
  'resolver',
  'storage',
  'transfer'
] as const

/** The chips as they read, in the collating order the component sorts its groups by. */
const ELEVEN_TYPES_AS_CHIPS = [
  'All12',
  'Folder crawler1',
  'Intake1',
  'Metadata1',
  'Notification1',
  'OAuth sign-in1',
  'Post-processing1',
  'Remote job1',
  'Resolver2',
  'Sign-in1',
  'Storage1',
  'Transfer backend1'
]

function plugin(type: string, index: number, executionCount = 0) {
  return {
    execution_count: executionCount,
    id: `com.example.${type}.${index}`,
    name: `${type} plugin ${index}`,
    description: `A ${type} plugin.`,
    author: 'Example',
    license: 'GPL-3.0-or-later',
    version: '1.0.0',
    api_version: '1.0.0',
    plugin_type: type,
    provider_slug: `${type}-${index}`,
    capabilities: [] as string[],
    domains: [] as string[],
    max_concurrent_downloads: 1,
    active: true
  }
}

/** One plugin per type, plus a second resolver so a filtered group holds more than one card. */
function inventoryFor(types: readonly string[]) {
  return [...types.map((type, index) => plugin(type, index + 1)), plugin('resolver', 2)]
}

/** The chip row, addressed the way a screen reader addresses it. */
function chipRow(): HTMLElement {
  return screen.getByRole('group', { name: pluginsCatalogue.installed.filter_label })
}

function chips(): HTMLElement[] {
  return within(chipRow()).getAllByRole('button')
}

/** A chip's visible text is its full group name followed by that group's count. */
function chipTexts(): string[] {
  return chips().map(chip => chip.textContent ?? '')
}

function chipNamed(label: string): HTMLElement {
  const found = chips().find(chip => (chip.textContent ?? '').replace(/\d+$/, '') === label)
  if (!found) throw new Error(`no chip labelled ${label} among ${chipTexts().join(', ')}`)
  return found
}

function cardTitles(): string[] {
  return screen.getAllByRole('heading', { level: 4 }).map(heading => heading.textContent ?? '')
}

/** One entry of `GET /api/v1/plugins/revocations`, as the service answers it. */
interface Revocation {
  digest: string
  plugin_id?: string | null
  plugin_name?: string | null
  version?: string | null
  reason?: string | null
  revoked_at: string
}

function serveInventory(installed: ReturnType<typeof plugin>[], revocations: Revocation[] = []): void {
  get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/plugins') return { data: { installed, incompatible: [] } }
    if (path === '/api/v1/plugins/revocations') return { data: revocations }
    return { data: { disabled_plugins: [] } }
  })
}

function mount(
  messages: Record<string, unknown> = { plugins: pluginsCatalogue },
  stubs: Record<string, unknown> = {}
) {
  return mountComponent(SettingsPluginsTab, { messages, stubs })
}

/** The plain `fetch` the trusted-key list and every removal go out over. */
let requests: ReturnType<typeof vi.fn>

function stubFetch(response: { ok: boolean, status: number, json: () => Promise<unknown> }): void {
  requests = vi.fn(async () => response)
  vi.stubGlobal('fetch', requests)
}

function resetMocks(): void {
  get.mockReset()
  post.mockReset()
  post.mockResolvedValue({ data: {} })
  remove.mockReset()
  remove.mockResolvedValue({ data: {} })
  responseError.mockReset()
  responseError.mockReturnValue('The service did not answer')
  confirmed.mockReset()
  confirmed.mockResolvedValue(true)
  serverMessage.mockReset()
  serverMessage.mockReturnValue(null)
  translated.mockReset()
  translated.mockReturnValue('')
  pluginEvent = null
  subscribedNames = []
  trustEvent = null
  stubFetch({ ok: true, status: 200, json: async () => [] })
}

describe('SettingsPluginsTab group filter', () => {
  beforeEach(resetMocks)

  it('names every installed group in full and keeps its count beside it', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(chips().length).toBe(INSTALLED_TYPES.length + 1))
    // Exactly the catalogue strings: no label is shortened, and none of them ends in the
    // ellipsis the tab bar produced once twelve entries had to share one line.
    expect(chipTexts()).toEqual(ELEVEN_TYPES_AS_CHIPS)
    expect(chipTexts().some(text => text.includes('…'))).toBe(false)
    // The row wraps instead of dividing one line, which is what keeps the labels whole.
    expect(chipRow().className).toContain('flex-wrap')
  })

  it('filters the cards to the chosen group and says which chip is pressed', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    expect(chipNamed('All').getAttribute('aria-pressed')).toBe('true')

    await fireEvent.click(chipNamed('Resolver'))

    await waitFor(() => expect(cardTitles()).toEqual(['resolver plugin 9', 'resolver plugin 2']))
    expect(chipNamed('Resolver').getAttribute('aria-pressed')).toBe('true')
    expect(chipNamed('All').getAttribute('aria-pressed')).toBe('false')
  })

  it('keeps the selection when the inventory is fetched again and a count moves', async () => {
    const full = inventoryFor(INSTALLED_TYPES)
    serveInventory(full)

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    await fireEvent.click(chipNamed('Resolver'))
    await waitFor(() => expect(cardTitles().length).toBe(2))

    // Removing a plugin replaces both lists with freshly fetched arrays, so every computed above
    // is rebuilt. The selection lives in its own ref and has to survive that, as it did before.
    serveInventory(full.filter(entry => entry.name !== 'resolver plugin 2'))
    const card = screen.getByText('resolver plugin 2').closest('article') as HTMLElement
    const remove = within(card)
      .getAllByRole('button')
      .find(button => button.textContent === 'Delete')
    expect(remove).toBeTruthy()
    await fireEvent.click(remove as HTMLElement)

    await waitFor(() => expect(cardTitles()).toEqual(['resolver plugin 9']))
    expect(chipNamed('Resolver').getAttribute('aria-pressed')).toBe('true')
    expect(chipNamed('Resolver').textContent).toBe('Resolver1')
  })

  it('does not collapse when a twelfth plugin type appears', async () => {
    // An invented world, standing in for the next one the project adds. The tab bar's failure
    // mode was that each new type shortened every existing label; here it is one more chip and,
    // on a narrow window, one more line.
    const catalogue = {
      ...pluginsCatalogue,
      type: { ...pluginsCatalogue.type, telemetry: 'Telemetry reporting' }
    }
    serveInventory(inventoryFor([...INSTALLED_TYPES, 'telemetry']))

    mount({ plugins: catalogue })

    await waitFor(() => expect(chips().length).toBe(INSTALLED_TYPES.length + 2))
    expect(chipTexts()).toEqual([
      'All13',
      'Folder crawler1',
      'Intake1',
      'Metadata1',
      'Notification1',
      'OAuth sign-in1',
      'Post-processing1',
      'Remote job1',
      'Resolver2',
      'Sign-in1',
      'Storage1',
      'Telemetry reporting1',
      'Transfer backend1'
    ])
    expect(chipTexts().some(text => text.includes('…'))).toBe(false)
    expect(chipRow().className).toContain('flex-wrap')

    await fireEvent.click(chipNamed('Telemetry reporting'))
    await waitFor(() => expect(cardTitles()).toEqual(['telemetry plugin 12']))
  })
})

/**
 * The second reported case (RD-108-10).
 *
 * "The plugin view says 44 while the package holds 43." It was not a display error: installing
 * never removes the older version, so one id stood there twice and the counter counted version
 * directories. These tests hold the answer to that — the count is a count of plugins, the
 * leftover version hangs under the plugin it belongs to, and getting rid of it is a confirmed,
 * refusable action rather than a silent delete.
 */
describe('SettingsPluginsTab superseded versions', () => {
  beforeEach(resetMocks)

  /** The number beside the "Local inventory" heading. */
  function inventoryCount(): string {
    const heading = screen.getByRole('heading', { level: 3, name: pluginsCatalogue.installed.title })
    const header = heading.parentElement?.parentElement as HTMLElement
    return (header.lastElementChild?.textContent ?? '').trim()
  }

  /** One plugin installed twice: the version that loads, and the one it replaced. */
  function inventoryWithLeftover() {
    const active = plugin('resolver', 2)
    return [
      ...inventoryFor(INSTALLED_TYPES),
      { ...active, version: '0.9.0', active: false }
    ]
  }

  function card(name: string): HTMLElement {
    return screen.getByText(name).closest('article') as HTMLElement
  }

  async function openLeftovers(name: string): Promise<HTMLElement> {
    const article = card(name)
    const toggle = within(article)
      .getAllByRole('button')
      .find(button => button.textContent === 'Superseded versions (1)')
    expect(toggle).toBeTruthy()
    expect(toggle?.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(toggle as HTMLElement)
    await waitFor(() => expect(within(article).getByText('v0.9.0')).toBeTruthy())
    return article
  }

  it('counts plugins rather than installed version directories', async () => {
    serveInventory(inventoryWithLeftover())

    mount()

    // Thirteen version directories, twelve plugins — and the number a person compares with the
    // package they installed is the second one.
    await waitFor(() => expect(cardTitles().length).toBe(12))
    expect(inventoryCount()).toBe('12')
    expect(chipNamed('All').textContent).toBe('All12')
    expect(chipNamed('Resolver').textContent).toBe('Resolver2')
  })

  it('shows the leftover version under the plugin it belongs to, not as a card of its own', async () => {
    serveInventory(inventoryWithLeftover())

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const article = await openLeftovers('resolver plugin 2')
    // The card keeps naming the version that is loaded; the other one sits inside it.
    expect(within(article).getByText('v1.0.0')).toBeTruthy()
    expect(within(article).getByText(pluginsCatalogue.card.superseded)).toBeTruthy()
    expect(within(article).getByText(pluginsCatalogue.card.superseded_hint)).toBeTruthy()
  })

  it('asks before removing a leftover and sends the version that is being removed', async () => {
    serveInventory(inventoryWithLeftover())

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const article = await openLeftovers('resolver plugin 2')
    await fireEvent.click(within(article).getByRole('button', { name: 'Remove version 0.9.0' }))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    // Destructive, and therefore confirmed the way `design.md` requires of every such action.
    expect(confirmed.mock.calls[0]?.[0]).toMatchObject({ destructive: true, confirmIcon: 'i-lucide-trash-2' })
    const deletes = requests.mock.calls.filter(call => (call[1] as { method?: string })?.method === 'DELETE')
    expect(deletes.map(call => call[0])).toEqual(['/api/v1/plugins/com.example.resolver.2/0.9.0'])
  })

  it('shows the reason when the service refuses to remove a version still in use', async () => {
    // The sentence is the catalogue's own singular form, not an invented one: `locales.test.ts`
    // checks that the code translates and pluralises, and this checks that what comes back
    // reaches the reader.
    const refusal = (serverCatalogue.codes['plugin.version_in_use'].split(' | ')[0] ?? '')
      .replace('{names}', 'archive.bin')
    expect(refusal).toContain('archive.bin')
    serveInventory(inventoryWithLeftover())
    stubFetch({
      ok: false,
      status: 409,
      json: async () => ({ code: 'plugin.version_in_use', params: { count: '1', names: 'archive.bin' } })
    })
    serverMessage.mockReturnValue({ code: 'plugin.version_in_use' })
    translated.mockReturnValue(refusal)

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const article = await openLeftovers('resolver plugin 2')
    await fireEvent.click(within(article).getByRole('button', { name: 'Remove version 0.9.0' }))

    // The refusal is what the view says instead of the version disappearing.
    await waitFor(() => expect(screen.getByText(refusal)).toBeTruthy())
    expect(cardTitles().length).toBe(12)
  })
})

/**
 * Withdrawing one exact build (RD-108-31).
 *
 * The service can refuse a single package by its content digest without distrusting the key it
 * was signed with. Three things decide whether that is usable: the withdrawn build has to be
 * named the way a person knows it — by plugin and version, not by 64 hex characters — the screen
 * has to say that the refusal starts at the next start rather than now, and the same list has to
 * be able to take the withdrawal back. The fourth test holds the refusal path: a coded answer
 * from the service reaches the reader as its own sentence.
 */
describe('SettingsPluginsTab withdrawn packages', () => {
  beforeEach(resetMocks)

  /** `UModal` renders its content in named slots, which the shared passthrough stub drops. */
  const MODAL_STUB = { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }

  /** A withdrawal of the second resolver, which this machine has installed. */
  const INSTALLED_ENTRY: Revocation = {
    digest: '11'.repeat(32),
    plugin_id: 'com.example.resolver.2',
    plugin_name: 'resolver plugin 2',
    version: '1.0.0',
    reason: 'Ships a hoster that leaks the account name',
    revoked_at: '2026-09-18T10:00:00Z'
  }

  /** A withdrawal whose package is gone from this machine; only its digest still names it. */
  const REMOVED_ENTRY: Revocation = {
    digest: 'ab'.repeat(32),
    plugin_id: 'com.example.gone',
    plugin_name: 'gone plugin',
    version: '0.1.0',
    reason: null,
    revoked_at: '2026-09-17T09:00:00Z'
  }

  function section(): HTMLElement {
    const heading = screen.getByRole('heading', { level: 3, name: pluginsCatalogue.withdrawn.title })
    return heading.closest('section') as HTMLElement
  }

  function cardFor(name: string): HTMLElement {
    return screen.getByRole('heading', { level: 4, name }).closest('article') as HTMLElement
  }

  function revocationFetches(): number {
    return get.mock.calls.filter(call => call[0] === '/api/v1/plugins/revocations').length
  }

  /** The digest as the view groups it for the eye, in blocks of eight. */
  function grouped(digest: string): string {
    return (digest.match(/.{1,8}/g) ?? []).join(' ')
  }

  it('names the withdrawn build by plugin and version, and falls back to the digest only when it must', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES), [INSTALLED_ENTRY, REMOVED_ENTRY])

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const list = section()
    // The sentence somebody has to read before they conclude the feature is broken.
    expect(within(list).getByText(pluginsCatalogue.withdrawn.description)).toBeTruthy()

    // The installed one is named the way the card names it, and its digest stays out of the way.
    expect(within(list).getByText('resolver plugin 2')).toBeTruthy()
    expect(within(list).getByText(pluginsCatalogue.withdrawn.installed)).toBeTruthy()
    expect(within(list).queryByText(grouped(INSTALLED_ENTRY.digest))).toBeNull()
    expect(within(list).getByText(`Reason: ${INSTALLED_ENTRY.reason}`)).toBeTruthy()
    // And the card itself says the version it shows is the one being refused.
    expect(within(cardFor('resolver plugin 2')).getByText(pluginsCatalogue.card.withdrawn_badge)).toBeTruthy()

    // The one that is no longer installed has nothing else to be identified by.
    expect(within(list).getByText('gone plugin')).toBeTruthy()
    expect(within(list).getByText(pluginsCatalogue.withdrawn.not_installed)).toBeTruthy()
    expect(within(list).getByText(grouped(REMOVED_ENTRY.digest))).toBeTruthy()
  })

  it('withdraws one exact build by plugin and version, and reloads the list', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount({ plugins: pluginsCatalogue }, MODAL_STUB)

    await waitFor(() => expect(cardTitles().length).toBe(12))
    await fireEvent.click(within(cardFor('resolver plugin 2')).getByRole('button', {
      name: pluginsCatalogue.actions.withdraw
    }))

    // Both consequences are stated before the act, which is what the dialog is for: this one
    // build is refused from the next start on, and the signing key keeps every other plugin.
    await waitFor(() => expect(screen.getByText(pluginsCatalogue.withdraw.restart)).toBeTruthy())
    expect(screen.getByText(pluginsCatalogue.withdraw.key_untouched)).toBeTruthy()

    await fireEvent.update(screen.getByRole('textbox'), 'Leaks the account name')
    const before = revocationFetches()
    serveInventory(inventoryFor(INSTALLED_TYPES), [INSTALLED_ENTRY])
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.withdraw.confirm }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    // The version the card shows, not a digest the reader would have had to copy from somewhere.
    expect(post.mock.calls[0]).toEqual(['/api/v1/plugins/revocations', {
      body: { plugin_id: 'com.example.resolver.2', version: '1.0.0', reason: 'Leaks the account name' }
    }])
    await waitFor(() => expect(revocationFetches()).toBeGreaterThan(before))
    await waitFor(() =>
      expect(within(cardFor('resolver plugin 2')).getByText(pluginsCatalogue.card.withdrawn_badge)).toBeTruthy())
  })

  it('lifts a withdrawal by its digest and reloads the list', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES), [INSTALLED_ENTRY])

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const before = revocationFetches()
    serveInventory(inventoryFor(INSTALLED_TYPES))
    await fireEvent.click(within(section()).getByRole('button', { name: pluginsCatalogue.withdrawn.lift }))

    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1))
    expect(remove.mock.calls[0]).toEqual(['/api/v1/plugins/revocations/{digest}', {
      params: { path: { digest: INSTALLED_ENTRY.digest } }
    }])
    await waitFor(() => expect(revocationFetches()).toBeGreaterThan(before))
    // With the withdrawal gone the card stops claiming one.
    await waitFor(() =>
      expect(within(cardFor('resolver plugin 2')).queryByText(pluginsCatalogue.card.withdrawn_badge)).toBeNull())
  })

  it('shows the service’s own sentence when a withdrawal is refused', async () => {
    const refusal = serverCatalogue.codes['plugin.digest_already_revoked']
    serveInventory(inventoryFor(INSTALLED_TYPES))
    post.mockResolvedValue({ error: { code: 'plugin.digest_already_revoked' } })
    responseError.mockReturnValue(refusal)

    mount({ plugins: pluginsCatalogue }, MODAL_STUB)

    await waitFor(() => expect(cardTitles().length).toBe(12))
    await fireEvent.click(within(cardFor('resolver plugin 2')).getByRole('button', {
      name: pluginsCatalogue.actions.withdraw
    }))
    await waitFor(() => expect(screen.getByText(pluginsCatalogue.withdraw.restart)).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.withdraw.confirm }))

    // The coded answer, translated — not "Request failed" and not a raw status number.
    await waitFor(() => expect(screen.getByText(refusal)).toBeTruthy())
    expect(within(cardFor('resolver plugin 2')).queryByText(pluginsCatalogue.card.withdrawn_badge)).toBeNull()
  })
})

/**
 * The tab used to read its three lists on mount and then only after its own writes, so a key
 * trusted or revoked, or a package withdrawn, in a second tab or by the service itself left
 * this one showing the state from when it was opened until somebody reloaded the page.
 */
describe('SettingsPluginsTab reacting to the plugin events', () => {
  beforeEach(resetMocks)

  const WITHDRAWAL: Revocation = {
    digest: '22'.repeat(32),
    plugin_id: 'com.example.resolver.2',
    plugin_name: 'resolver plugin 2',
    version: '1.0.0',
    reason: null,
    revoked_at: '2026-09-18T10:00:00Z'
  }

  function cardFor(name: string): HTMLElement {
    return screen.getByRole('heading', { level: 4, name }).closest('article') as HTMLElement
  }

  /**
   * The channel, not just the reaction. `/api/v1/plugins` costs `Admin`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin_catalog.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    expect(subscribedNames).toEqual(['plugin.changed', 'plugin_trust.changed'])
  })

  it('re-reads the trust store and the withdrawals on plugin_trust.changed', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    expect(within(cardFor('resolver plugin 2')).queryByText(pluginsCatalogue.card.withdrawn_badge)).toBeNull()
    const keyReadsBefore = requests.mock.calls.length

    // The withdrawal happened elsewhere; this tab is only told that something changed. It
    // arrives on the trust event and not on `plugin.changed`, because its payload names a
    // digest — a `Secrets` fact that an administration subscriber may not be handed.
    serveInventory(inventoryFor(INSTALLED_TYPES), [WITHDRAWAL])
    trustEvent?.({ data: JSON.stringify({ payload: { resource: 'plugin_revocation', digest: WITHDRAWAL.digest } }) } as MessageEvent<string>)

    // The badge is the proof the withdrawals were fetched again and applied to the card.
    await waitFor(
      () => expect(within(cardFor('resolver plugin 2')).getByText(pluginsCatalogue.card.withdrawn_badge)).toBeTruthy(),
      { timeout: 2000 }
    )
    // And the trust store with it: a revoked key still listed as trusted is the one thing this
    // list must never claim.
    expect(requests.mock.calls.length).toBeGreaterThan(keyReadsBefore)
  })

  it('re-reads the inventory on plugin.changed', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))

    // A version was removed in a second tab. The administration writes keep the original
    // event, so this is the name the installed list has to be listening on.
    serveInventory(inventoryFor(INSTALLED_TYPES.slice(0, -1)))
    pluginEvent?.({ data: JSON.stringify({ payload: { resource: 'plugin' } }) } as MessageEvent<string>)

    await waitFor(() => expect(cardTitles().length).toBe(11), { timeout: 2000 })
  })

  it('coalesces a burst from both events into one reload', async () => {
    serveInventory(inventoryFor(INSTALLED_TYPES))

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(12))
    const before = get.mock.calls.filter(call => call[0] === '/api/v1/plugins').length
    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) {
      pluginEvent?.(event)
      trustEvent?.(event)
    }

    await waitFor(
      () => expect(get.mock.calls.filter(call => call[0] === '/api/v1/plugins').length).toBe(before + 1),
      { timeout: 2000 }
    )
    // Installing a package emits more than one event, and the two names share one debounce
    // because the three lists are shown as one state; ten of them must not cost thirty round
    // trips.
    expect(get.mock.calls.filter(call => call[0] === '/api/v1/plugins').length).toBe(before + 1)
  })
})
/**
 * RD-120-28 — no button with nothing behind it.
 *
 * Every plugin card carried a diagnostics accordion whether or not that plugin had ever been
 * invoked, and opening an empty one said "nothing recorded yet" — a click spent to learn less
 * than before, because that sentence cannot tell *never ran* from *ran and was unremarkable*.
 *
 * The hard part was never the `v-if`: the entries are fetched when somebody opens the panel,
 * so before the first open the card had no way of knowing whether there were any. The answer
 * is a count the inventory now carries — a number, never an entry — which is why the second
 * test below asserts that mounting still sends no request to the executions endpoint. If that
 * assertion ever goes, load-on-demand has been undone by accident and these tests are the only
 * thing that would say so.
 */
describe('SettingsPluginsTab diagnostics accordion', () => {
  beforeEach(resetMocks)

  /** One recorded invocation, shaped as `GET /api/v1/plugins/{id}/executions` answers it. */
  const ENTRY = {
    id: '019d0000-0000-7000-8000-0000000000aa',
    plugin_id: 'com.example.resolver.1',
    plugin_version: '1.0.0',
    plugin_type: 'resolver',
    operation: 'resolve',
    correlation_id: '019d0000-0000-7000-8000-0000000000bb',
    outcome: 'ok',
    error_class: null,
    message: null,
    started_at: '2026-09-22T10:00:00Z',
    duration_ms: 12
  }

  /** A resolver that has run twice and a notifier that has never run. */
  const QUIET = plugin('notifier', 1)
  const BUSY = { ...plugin('resolver', 1, 2), id: ENTRY.plugin_id }

  function serve(entries: unknown[] = [ENTRY]): void {
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/plugins') return { data: { installed: [BUSY, QUIET], incompatible: [] } }
      if (path === '/api/v1/plugins/revocations') return { data: [] }
      if (path === '/api/v1/plugins/{id}/executions') return { data: entries }
      return { data: { disabled_plugins: [] } }
    })
  }

  function cardFor(name: string): HTMLElement {
    return screen.getByRole('heading', { level: 4, name }).closest('article') as HTMLElement
  }

  function accordionIn(article: HTMLElement): HTMLElement | null {
    return within(article).queryByRole('button', { name: pluginsCatalogue.diagnostics.title })
  }

  function executionRequests(): unknown[][] {
    return get.mock.calls.filter(call => call[0] === '/api/v1/plugins/{id}/executions')
  }

  it('offers no accordion on a plugin that has recorded nothing', async () => {
    serve()

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(2))
    // The card is there in full — only the promise of something to unfold is gone.
    expect(accordionIn(cardFor('notifier plugin 1'))).toBeNull()
    expect(within(cardFor('notifier plugin 1')).getByText(pluginsCatalogue.actions.disable)).toBeTruthy()
    // And it is absent, not disabled: a greyed control still has to be read and dismissed.
    expect(within(cardFor('notifier plugin 1')).queryByText(pluginsCatalogue.diagnostics.title)).toBeNull()
  })

  it('keeps the accordion where there is something behind it, and opens it on demand', async () => {
    serve()

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(2))
    const toggle = accordionIn(cardFor('resolver plugin 1'))
    expect(toggle).toBeTruthy()
    // The count came with the inventory, so nothing was fetched to decide that the button
    // belongs here. This is the assertion that guards the 2026 decision.
    expect(executionRequests().length).toBe(0)

    await fireEvent.click(toggle as HTMLElement)

    await waitFor(() => expect(within(cardFor('resolver plugin 1')).getByText(ENTRY.operation)).toBeTruthy())
    expect(executionRequests().length).toBe(1)
    expect(executionRequests()[0]?.[1]).toEqual({ params: { path: { id: BUSY.id } } })
    // The entry itself, not just the panel: outcome badge and correlation id are what a user
    // is asked to quote.
    expect(within(cardFor('resolver plugin 1')).getByText(pluginsCatalogue.diagnostics.outcome.ok)).toBeTruthy()
    expect(within(cardFor('resolver plugin 1')).getByText(ENTRY.correlation_id)).toBeTruthy()
  })

  it('says it is loading rather than showing an empty panel while the entries are in flight', async () => {
    // A holder rather than a bare `let`: the resolver is assigned inside the mock, and the
    // narrowing of a captured local would make the call below unreachable to the checker.
    const held: { release?: (value: { data: unknown[] }) => void } = {}
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/plugins') return { data: { installed: [BUSY, QUIET], incompatible: [] } }
      if (path === '/api/v1/plugins/revocations') return { data: [] }
      if (path === '/api/v1/plugins/{id}/executions') {
        return await new Promise<{ data: unknown[] }>(resolve => { held.release = resolve })
      }
      return { data: { disabled_plugins: [] } }
    })

    mount()

    await waitFor(() => expect(cardTitles().length).toBe(2))
    await fireEvent.click(accordionIn(cardFor('resolver plugin 1')) as HTMLElement)

    // The open panel is never silent: "nothing recorded yet" used to render here for as long
    // as the request took, and it was false every time it did.
    await waitFor(() => expect(within(cardFor('resolver plugin 1')).getByText(commonCatalogue.data.loading)).toBeTruthy())

    held.release?.({ data: [ENTRY] })

    await waitFor(() => expect(within(cardFor('resolver plugin 1')).getByText(ENTRY.operation)).toBeTruthy())
    expect(within(cardFor('resolver plugin 1')).queryByText(commonCatalogue.data.loading)).toBeNull()
  })
})
