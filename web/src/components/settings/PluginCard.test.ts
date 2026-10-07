/**
 * One installed plugin as a card (RD-180-22): who it is in the header, what it may do and which
 * versions it has as labelled rows, and the actions on the whole plugin in the footer, where they
 * stay whatever the card holds above them. The behaviour behind each action is the tab's and is
 * tested there (`SettingsPluginsTab.test.ts`); this holds the card to its shape and its wiring.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import commonCatalogue from '@/locales/en/common.json'
import pluginsCatalogue from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({
  api: { POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => 'refused',
  resultMessage: () => 'saved'
}))
vi.mock('@/i18n', () => ({
  currentLocale: () => 'en',
  i18n: { global: { d: (value: Date) => value.toISOString(), t: (key: string) => key } }
}))
vi.mock('@/i18n/plugins', () => ({ providerText: () => undefined }))

const { default: PluginCard } = await import('./PluginCard.vue')

const ID = '019d0000-0000-7000-8000-000000000132'

function plugin(overrides: Record<string, unknown> = {}) {
  return {
    execution_count: 0,
    id: ID,
    name: 'MEGA',
    description: 'Resolves MEGA file links.',
    author: 'rDownloader project',
    license: 'MIT',
    homepage: 'https://example.org/mega',
    version: '0.1.7',
    api_version: '0.9.0',
    plugin_type: 'stream-transform',
    provider_slug: 'mega',
    capabilities: ['net_http', 'secrets:mega_session'],
    domains: ['mega.nz', 'g.api.mega.co.nz'],
    max_concurrent_downloads: 1,
    active: true,
    ...overrides
  }
}

function lifecycle(overrides: Record<string, unknown> = {}) {
  return {
    plugin_id: ID,
    active_version: '0.1.7',
    running_version: '0.1.7',
    staged_version: null,
    previous_version: null,
    update_policy: 'manual',
    restart_required: false,
    ...overrides
  }
}

function mount(props: Record<string, unknown> = {}) {
  return mountComponent(PluginCard, {
    messages: { plugins: pluginsCatalogue },
    props: {
      plugin: plugin(),
      superseded: [],
      disabled: false,
      isWithdrawn: () => false,
      lifecycle: lifecycle(),
      releaseNotes: [],
      supersededOpen: false,
      diagnosticsOpen: false,
      diagnosticsLoading: false,
      executions: [],
      ...props
    }
  })
}

function footer(): HTMLElement {
  return document.querySelector('[data-plugin-actions]') as HTMLElement
}

describe('PluginCard', () => {
  it('names the plugin in the header with its badges and one meta line', () => {
    mount()

    const article = screen.getByRole('article')
    expect(within(article).getByRole('heading', { level: 4, name: 'MEGA' })).toBeTruthy()
    expect(within(article).getByText('v0.1.7')).toBeTruthy()
    expect(within(article).getByText('mega')).toBeTruthy()
    expect(within(article).getByText(pluginsCatalogue.type['stream-transform'])).toBeTruthy()
    expect(within(article).getByText('ABI 0.9.0')).toBeTruthy()
    const meta = document.querySelector('[data-plugin-meta]') as HTMLElement
    // The dots are drawn before an item, never as an item of their own that could end a line.
    expect(Array.from(meta.children, item => item.textContent)).toEqual(['by rDownloader project', 'MIT', pluginsCatalogue.card.homepage, ID])
    expect(meta.textContent).not.toContain('·')
    expect(within(meta).getByRole('link', { name: pluginsCatalogue.card.homepage }).getAttribute('rel')).toBe('noopener noreferrer')
  })

  it('pins the icon to the first column and starts the description under the title', () => {
    mount()

    // The header is a grid; a cell that only spans would reset its start column and push the
    // auto-placed icon to the far right (RD-180-22, second visual check).
    const icon = document.querySelector('[data-plugin-icon]') as HTMLElement
    expect(icon.parentElement?.firstElementChild).toBe(icon)
    expect(icon.className.split(' ')).toEqual(expect.arrayContaining(['col-start-1', 'row-start-1']))
    const about = document.querySelector('[data-plugin-about]') as HTMLElement
    expect(about.className.split(' ')).toEqual(expect.arrayContaining(['col-start-2', 'sm:col-[2/span_2]']))
    expect(about.className).not.toMatch(/(^|\s)(sm:)?col-span-/)
  })

  it('lists what the plugin may do under one label, grants before hosts', () => {
    mount()

    const row = document.querySelector('[data-plugin-permissions]') as HTMLElement
    expect(row.firstElementChild?.textContent).toBe(pluginsCatalogue.card.capabilities)
    expect(row.textContent).toContain(pluginsCatalogue.capability.net_http)
    expect(row.textContent).toContain('mega.nz')
    expect(row.textContent?.indexOf(pluginsCatalogue.capability.net_http)).toBeLessThan(row.textContent?.indexOf('mega.nz') ?? 0)
  })

  it('shows the first eight hosts of a long list and unfolds the rest', async () => {
    const hosts = Array.from({ length: 20 }, (_, index) => `host${index}.example`)
    mount({ plugin: plugin({ domains: hosts }) })

    const row = document.querySelector('[data-plugin-hosts]') as HTMLElement
    expect(within(row).getByText('host7.example')).toBeTruthy()
    expect(within(row).queryByText('host8.example')).toBeNull()
    const toggle = within(row).getByRole('button', { name: '+12 more' })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')

    await fireEvent.click(toggle)
    expect(within(row).getByText('host19.example')).toBeTruthy()
    await fireEvent.click(within(row).getByRole('button', { name: pluginsCatalogue.card.fewer_hosts }))
    expect(within(row).queryByText('host8.example')).toBeNull()
  })

  it('offers no toggle for a host list that fits', () => {
    mount()

    expect(within(document.querySelector('[data-plugin-hosts]') as HTMLElement).queryByRole('button')).toBeNull()
  })

  it('says so when a plugin may do nothing at all', () => {
    mount({ plugin: plugin({ capabilities: [], domains: [] }) })

    expect(document.querySelector('[data-plugin-permissions]')?.textContent).toContain(pluginsCatalogue.preview.no_permissions)
  })

  it('names the running version beside the version controls', () => {
    mount()

    const row = document.querySelector('[data-plugin-versions]') as HTMLElement
    expect(within(row).getByText('Running: v0.1.7')).toBeTruthy()
    expect(within(row).queryByText('Restart to apply')).toBeNull()
    expect(within(row).getByRole('switch', { name: pluginsCatalogue.versions.auto_update })).toBeTruthy()
  })

  it('keeps the running version and the one of the next start apart until a restart', () => {
    mount({ lifecycle: lifecycle({ active_version: '0.1.6', previous_version: '0.1.7', restart_required: true }) })

    const row = document.querySelector('[data-plugin-versions]') as HTMLElement
    expect(within(row).getByText('Running: v0.1.7')).toBeTruthy()
    expect(within(row).getByText('From the next start: v0.1.6')).toBeTruthy()
    expect(within(row).getByText('Restart to apply')).toBeTruthy()
  })

  it('folds the superseded versions away and asks the tab to unfold them', async () => {
    const old = plugin({ version: '0.1.4', active: false })
    const view = mount({ superseded: [old] })

    const toggle = screen.getByRole('button', { name: 'Superseded versions (1)' })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    // The select beside it offers v0.1.4 too; the superseded list itself is not there yet.
    expect(screen.queryByText(pluginsCatalogue.card.superseded_hint)).toBeNull()
    await fireEvent.click(toggle)
    expect(view.emitted('toggleSuperseded')).toHaveLength(1)

    await view.rerender({ supersededOpen: true })
    expect(screen.getByText(pluginsCatalogue.card.superseded_hint)).toBeTruthy()
    // One bordered group, a row per version, not a box each.
    expect(screen.getByText('v0.1.4', { selector: 'span' }).closest('.divide-y')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.card.superseded)).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: 'Withdraw version 0.1.4' }))
    expect(view.emitted('withdraw')).toEqual([[old]])
    await fireEvent.click(screen.getByRole('button', { name: 'Remove version 0.1.4' }))
    expect(view.emitted('removeSuperseded')).toEqual([[old]])
  })

  it('removes every superseded version at once from the unfolded section (RD-1140-04)', async () => {
    const versions = [plugin({ version: '0.1.4', active: false }), plugin({ version: '0.1.5', active: false })]
    const view = mount({ superseded: versions })

    // Folded, the action is not in reach: it belongs to the list it removes.
    expect(screen.queryByRole('button', { name: pluginsCatalogue.card.remove_all_superseded })).toBeNull()
    await view.rerender({ supersededOpen: true })
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.card.remove_all_superseded }))
    expect(view.emitted('removeAllSuperseded')).toHaveLength(1)
    expect(view.emitted('removeSuperseded')).toBeUndefined()
  })

  it('shows no version row for a plugin with neither a lifecycle nor a superseded version', () => {
    mount({ lifecycle: undefined })

    expect(document.querySelector('[data-plugin-versions]')).toBeNull()
  })

  it('offers diagnostics only for a plugin that has run, folded until the tab opens them', async () => {
    const quiet = mount()
    expect(screen.queryByRole('button', { name: pluginsCatalogue.diagnostics.title })).toBeNull()
    quiet.unmount()

    const view = mount({ plugin: plugin({ execution_count: 3 }), diagnosticsLoading: true })
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.diagnostics.title }))
    expect(view.emitted('toggleDiagnostics')).toHaveLength(1)
    expect(screen.queryByText(commonCatalogue.data.loading)).toBeNull()
    await view.rerender({ diagnosticsOpen: true })
    expect(screen.getByText(commonCatalogue.data.loading)).toBeTruthy()
  })

  it('ends with Delete on the left and Withdraw and Disable on the right, each wired', async () => {
    const view = mount()

    const buttons = within(footer()).getAllByRole('button').map(button => button.textContent)
    expect(buttons).toEqual([commonCatalogue.actions.delete, pluginsCatalogue.actions.withdraw, pluginsCatalogue.actions.disable])
    // Icon-only on a phone: the name the two ghost buttons drop there stays in aria-label.
    expect(within(footer()).getByRole('button', { name: commonCatalogue.actions.delete }).getAttribute('title')).toBe(commonCatalogue.actions.delete)
    // The footer is the card's last region, after every row the body may grow.
    const article = screen.getByRole('article')
    expect(article.lastElementChild?.contains(footer())).toBe(true)

    await fireEvent.click(within(footer()).getByRole('button', { name: commonCatalogue.actions.delete }))
    expect(view.emitted('remove')).toHaveLength(1)
    await fireEvent.click(within(footer()).getByRole('button', { name: pluginsCatalogue.actions.withdraw }))
    expect(view.emitted<[{ id: string, version: string }]>('withdraw')[0]?.[0]).toMatchObject({ id: ID, version: '0.1.7' })
    await fireEvent.click(within(footer()).getByRole('button', { name: pluginsCatalogue.actions.disable }))
    expect(view.emitted('setEnabled')).toEqual([[false]])
  })

  it('offers Enable for a disabled plugin and no second withdrawal of a withdrawn one', async () => {
    const view = mount({ disabled: true, isWithdrawn: () => true })

    const buttons = within(footer()).getAllByRole('button').map(button => button.textContent)
    expect(buttons).toEqual([commonCatalogue.actions.delete, pluginsCatalogue.actions.enable])
    await fireEvent.click(within(footer()).getByRole('button', { name: pluginsCatalogue.actions.enable }))
    expect(view.emitted('setEnabled')).toEqual([[true]])
  })
})
