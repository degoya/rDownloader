import type { CommandPaletteItem } from '@nuxt/ui'
// The real key handling, not a copy: this subpath needs nothing but Vue and VueUse, unlike the
// `@nuxt/ui/composables` barrel that `useAppShortcuts.ts` imports (see `useAppShortcuts.test.ts`).
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { fireEvent, render } from '@testing-library/vue'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { MAIN_VIEWS, PALETTE_FUSE, buildPaletteGroups, keepFocusOnClose, openPalette, openSettingsAnchor, openSettingsEntry, paletteOpen } from './searchPalette'
import { SHORTCUT_DEFINITIONS, registeredShortcuts } from './shortcutDefinitions'
import { i18n, setLocale } from '@/i18n'
import { router } from '@/router'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { loadEveryLocale } from '@/test/locales'

beforeAll(loadEveryLocale)

const t = (key: string): string => i18n.global.t(key)

function items(): CommandPaletteItem[] {
  return buildPaletteGroups(t).flatMap(group => group.items ?? [])
}

/**
 * The ids of the items whose searched text holds `term`. The fields are the ones the palette
 * hands to Fuse — its defaults and `PALETTE_FUSE` — so a term found here is one the palette can
 * match; how Fuse ranks the matches is Nuxt UI's business, not this table's.
 */
function find(term: string): string[] {
  const fields = ['label', 'description', 'suffix', ...PALETTE_FUSE.fuseOptions.keys]
  const needle = term.toLowerCase()
  return items()
    .filter(item => fields.some(field => String(item[field] ?? '').toLowerCase().includes(needle)))
    .map(item => String(item.id))
}

function entry(id: string) {
  const found = SETTINGS_SEARCH_ENTRIES.find(candidate => candidate.id === id)
  if (!found) throw new Error(`no entry ${id}`)
  return found
}

afterEach(async () => {
  await setLocale('en')
  document.body.innerHTML = ''
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('the palette content', () => {
  it('offers the main views, the settings pages and the settings, in that order', () => {
    const groups = buildPaletteGroups(t)
    expect(groups.map(group => group.id)).toEqual(['views', 'pages', 'settings'])
    expect(groups[0]?.items?.map(item => item.kbds)).toEqual(MAIN_VIEWS.map(view => [view.key]))
    for (const view of MAIN_VIEWS) expect(router.resolve(view.path).matched.length).toBeGreaterThan(0)
  })

  it('shows the views in sidebar order with the digits the shortcut catalogue binds', () => {
    const digits = SHORTCUT_DEFINITIONS.filter(definition => definition.group === 'navigation').map(definition => definition.keys)
    expect(MAIN_VIEWS.map(view => view.key)).toEqual(digits)
  })

  it('finds a setting by its translated title in the current language', async () => {
    await setLocale('de')
    expect(find('Erlaubte Hostnamen')).toContain('setting:security.allowed_hosts')
    expect(find('Allowed host names')).not.toContain('setting:security.allowed_hosts')
    await setLocale('en')
    expect(find('allowed host names')).toContain('setting:security.allowed_hosts')
  })

  it('finds by synonym and by names that are the same in every language', async () => {
    await setLocale('de')
    expect(find('Dunkelmodus')).toContain('setting:interface.theme')
    expect(find('Passphrase')).toEqual(expect.arrayContaining(['setting:backup.export_passphrase', 'setting:backup.full_passphrase']))
    expect(find('ntfy')).toEqual(expect.arrayContaining(['page:notifications', 'setting:notifications.targets']))
    expect(find('7-Zip')).toEqual(expect.arrayContaining(['page:tools', 'setting:tools.status']))
    expect(find('rclone')).toContain('setting:postprocess.rclone_executable')
    expect(find('Proxy')).toContain('setting:network.proxies')
    expect(find('Bandbreite')).toContain('page:bandwidth')
    expect(find('Captcha')).toContain('page:captcha')
    expect(find('Hotfolder')).toContain('page:hotfolders')
  })

  // RD-1120-21: what left General is found where it is now, and only there.
  it('finds the settings that left General at their new place', async () => {
    await setLocale('de')
    expect(find('Admin')).toContain('setting:security.admin_login')
    expect(find('frei')).toContain('setting:routing.minimum_free')
    expect(find('Kollision')).toContain('setting:routing.collision')
    expect(find('NNTP')).toEqual(expect.arrayContaining(['setting:usenet.nntp_connections', 'setting:usenet.nntp_parallel_files']))
    expect(find('Limit')).toEqual(expect.arrayContaining(['setting:bandwidth.speed_limit', 'setting:bandwidth.upload_limit']))
    expect(find('Port')).toContain('setting:security.ui_port')
    expect(find('parallele Downloads')).toContain('setting:general.active_files')
    const ids = items().map(item => String(item.id))
    for (const old of ['admin_login', 'minimum_free', 'collision', 'speed_limit', 'ui_port']) {
      expect(ids).not.toContain(`setting:general.${old}`)
    }
  })
})

describe('choosing a result', () => {
  beforeEach(() => {
    vi.spyOn(router, 'push').mockResolvedValue(undefined)
    Element.prototype.scrollIntoView = vi.fn()
  })

  it('opens a main view', () => {
    const item = items().find(candidate => candidate.id === 'view:/linkgrabber')
    item?.onSelect?.(new Event('select'))
    expect(router.push).toHaveBeenCalledWith('/linkgrabber')
  })

  it('opens a card on its routing sub-tab', () => {
    const item = items().find(candidate => candidate.id === 'setting:routing.rules')
    item?.onSelect?.(new Event('select'))
    expect(router.push).toHaveBeenCalledWith({ path: '/settings/routing', query: { tab: 'rules' } })
  })

  // RD-180-15: the plugins page became tabs; the trusted keys sit on the last one. The panel is
  // mounted but hidden until the address has switched the tab, and only then is it scrolled to.
  it('opens a card on a sub-tab that is still hidden, and waits for the tab to show it', async () => {
    document.body.innerHTML = '<div role="tabpanel" hidden><section data-settings-anchor="plugins.keys"></section></div>'
    setTimeout(() => document.querySelector('[role="tabpanel"]')?.removeAttribute('hidden'), 120)
    const scrolled = vi.mocked(Element.prototype.scrollIntoView)
    const opened = openSettingsEntry(entry('plugins.keys'))
    expect(router.push).toHaveBeenCalledWith({ path: '/settings/plugins', query: { tab: 'trust' } })
    await new Promise(resolve => setTimeout(resolve, 60))
    expect(scrolled).not.toHaveBeenCalled()
    expect(await opened).toBe(true)
    expect(scrolled).toHaveBeenCalledTimes(1)
    expect(document.querySelector('[data-settings-anchor]')?.hasAttribute('data-search-highlight')).toBe(true)
  })

  it('leads an anchor that left General to its field at the new place (RD-1120-21)', async () => {
    document.body.innerHTML = '<div data-settings-anchor="security.admin_login"><button type="button" role="switch"></button></div>'
    expect(await openSettingsAnchor('general.admin_login')).toBe(true)
    expect(router.push).toHaveBeenCalledWith({ path: '/settings/security', query: { tab: 'signin' } })
    expect(document.activeElement).toBe(document.querySelector('[role="switch"]'))
    expect(await openSettingsAnchor('general.nonsense')).toBe(false)
  })

  it('scrolls to a field, marks it and moves the focus into it', async () => {
    document.body.innerHTML = '<div data-settings-anchor="security.allowed_hosts"><label>x</label><textarea></textarea></div>'
    expect(await openSettingsEntry(entry('security.allowed_hosts'))).toBe(true)
    expect(router.push).toHaveBeenCalledWith({ path: '/settings/security', query: { tab: 'proxy' } })
    const anchor = document.querySelector('[data-settings-anchor]')
    expect(anchor?.hasAttribute('data-search-highlight')).toBe(true)
    expect(Element.prototype.scrollIntoView).toHaveBeenCalledWith({ block: 'center', behavior: 'smooth' })
    expect(document.activeElement).toBe(document.querySelector('textarea'))
  })

  it('leaves the focus where it is for a card, and does not animate the scroll without motion', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query.includes('reduce') }))
    document.body.innerHTML = '<button id="before"></button><section data-settings-anchor="backup.full"><input /></section>'
    document.querySelector<HTMLElement>('#before')?.focus()
    expect(await openSettingsEntry(entry('backup.full'))).toBe(true)
    expect(Element.prototype.scrollIntoView).toHaveBeenCalledWith({ block: 'center', behavior: 'auto' })
    expect(document.activeElement?.id).toBe('before')
  })

  it('keeps the closing palette from taking the focus back out of a found field, and only then', async () => {
    document.body.innerHTML = '<div data-settings-anchor="general.retries"><input /></div>'
    await openSettingsEntry(entry('general.retries'))
    const first = new Event('focus.autoFocusOnUnmount', { cancelable: true })
    keepFocusOnClose(first)
    expect(first.defaultPrevented).toBe(true)
    const second = new Event('focus.autoFocusOnUnmount', { cancelable: true })
    keepFocusOnClose(second)
    expect(second.defaultPrevented).toBe(false)

    document.body.innerHTML = '<section data-settings-anchor="backup.full"></section>'
    await openSettingsEntry(entry('backup.full'))
    const afterCard = new Event('focus.autoFocusOnUnmount', { cancelable: true })
    keepFocusOnClose(afterCard)
    expect(afterCard.defaultPrevented).toBe(false)
  })

  it('waits for a page that renders late and gives up on one that never does', async () => {
    setTimeout(() => {
      document.body.innerHTML = '<section data-settings-anchor="system.logs"></section>'
    }, 120)
    expect(await openSettingsEntry(entry('system.logs'))).toBe(true)
    const { revealAnchor } = await import('@/utils/revealAnchor')
    expect(await revealAnchor('nowhere', { focus: false, timeoutMs: 100 })).toBe(false)
  })
})

describe('the shortcuts that open the palette', () => {
  /**
   * The app binds `registeredShortcuts()`; `UDashboardSearch` binds Ctrl/Cmd+K itself with
   * `usingInput: true` (its default `shortcut`, see its source). Both go through the same
   * `defineShortcuts` here, so what is tested is the real rule for text fields.
   */
  function mountShortcuts() {
    const host = defineComponent({
      setup() {
        defineShortcuts({ ...registeredShortcuts(), meta_k: { usingInput: true, handler: openPalette } })
        return () => h('div', [h('input', { 'data-testid': 'field' }), h('button', { 'data-testid': 'elsewhere' })])
      }
    })
    return render(host)
  }

  beforeEach(() => {
    paletteOpen.value = false
    vi.spyOn(router, 'push').mockResolvedValue(undefined)
  })

  it('lists `/` and Ctrl/Cmd+K in the help, and binds only `/` itself', () => {
    const search = SHORTCUT_DEFINITIONS.filter(definition => definition.descriptionKey === 'common.shortcuts.open_search')
    expect(search.map(definition => definition.keys)).toEqual(['/', 'meta_k'])
    expect(Object.keys(registeredShortcuts())).toContain('/')
    expect(Object.keys(registeredShortcuts())).not.toContain('meta_k')
  })

  it('opens on `/` outside a text field, but never while one is being typed in', async () => {
    const view = mountShortcuts()
    const field = view.getByTestId('field')
    field.focus()
    await fireEvent.keyDown(field, { key: '/' })
    expect(paletteOpen.value).toBe(false)

    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: '/' })
    expect(paletteOpen.value).toBe(true)
  })

  it('opens on Ctrl+K even from inside a text field', async () => {
    const view = mountShortcuts()
    const field = view.getByTestId('field')
    field.focus()
    await fireEvent.keyDown(field, { key: 'k', ctrlKey: true })
    expect(paletteOpen.value).toBe(true)
  })
})
