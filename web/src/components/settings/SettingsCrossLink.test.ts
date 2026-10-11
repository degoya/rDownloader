/**
 * The links between settings that work together (RD-1120-23): each names a search anchor, leads
 * to that anchor's page and tab, brings it into view, and the notice of a switched-off service
 * leads to its switch the same way.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import en from '@/locales/en/settings.json'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'
import { revealAnchor } from '@/utils/revealAnchor'

import SettingsCrossLink from './SettingsCrossLink.vue'
import SettingsServiceOffAlert from './SettingsServiceOffAlert.vue'

vi.mock('@/utils/revealAnchor', () => ({ revealAnchor: vi.fn(async () => true) }))

/** Every component and view template, read as text: the links live there. */
const sources: Record<string, string> = {
  ...import.meta.glob<string>('@/components/**/*.vue', { eager: true, query: '?raw', import: 'default' }),
  ...import.meta.glob<string>('@/views/**/*.vue', { eager: true, query: '?raw', import: 'default' })
}

describe('SettingsCrossLink', () => {
  beforeEach(() => vi.mocked(revealAnchor).mockClear())

  it('names the page and the card, and leads to the page and its tab', () => {
    mountComponent(SettingsCrossLink, { messages: { settings: en }, props: { anchor: 'network.proxies' } })
    const link = screen.getByRole('link')
    expect(link.textContent?.trim()).toBe(`${en.tabs.network} › ${en.proxy.list_title}`)
    expect(link.getAttribute('href')).toBe('/settings/network?tab=proxies')
    expect(screen.getByText(en.cross_link.see_also)).toBeTruthy()
  })

  it('takes its own lead and a more exact title', () => {
    mountComponent(SettingsCrossLink, {
      messages: { settings: en },
      props: { anchor: 'bandwidth.upload_limit', lead: en.cross_link.program_path, titleKey: 'settings.upload_limit.label' }
    })
    expect(screen.getByRole('link').textContent?.trim()).toBe(`${en.tabs.bandwidth} › ${en.upload_limit.label}`)
    expect(screen.getByText(en.cross_link.program_path)).toBeTruthy()
  })

  it('brings a field into view with the focus, a card without', async () => {
    mountComponent(SettingsCrossLink, { messages: { settings: en }, props: { anchor: 'postprocess.rclone_executable' } })
    expect(screen.getByRole('link').getAttribute('href')).toBe('/settings/tools?tab=paths')
    await fireEvent.click(screen.getByRole('link'))
    expect(revealAnchor).toHaveBeenCalledWith('postprocess.rclone_executable', { focus: true })
  })

  it('draws nothing for an anchor the registry does not know', () => {
    const { container } = mountComponent(SettingsCrossLink, { messages: { settings: en }, props: { anchor: 'nowhere.at_all' } })
    expect(container.querySelector('[data-settings-link]')).toBeNull()
  })

  // A card that moves takes its registry row along, so a link can only break by naming an
  // anchor that no longer exists — which this catches before a reader does.
  it('names only anchors the search registry has', () => {
    const ids = new Set(SETTINGS_SEARCH_ENTRIES.map(entry => entry.id))
    const named = Object.entries(sources).flatMap(([path, source]) =>
      [...source.matchAll(/<SettingsCrossLink\b[^>]*?\banchor="([^"]+)"/g)].map(match => ({ path, anchor: match[1] ?? '' })))
    expect(named.length).toBeGreaterThan(10)
    expect(named.filter(link => !ids.has(link.anchor))).toEqual([])
  })
})

describe('SettingsServiceOffAlert', () => {
  it('says which service is off and leads to its switch', async () => {
    mountComponent(SettingsServiceOffAlert, { messages: { settings: en }, props: { service: 'gallery', enabled: false } })
    expect(screen.getByText(en.services.off.title.replace('{name}', en.services.gallery.label))).toBeTruthy()
    const open = screen.getByRole('button', { name: en.services.off.open })
    await fireEvent.click(open)
    expect(revealAnchor).toHaveBeenCalledWith('services.switches', { focus: false })
  })

  it('renders the notice and a link without an axe violation', async () => {
    const notice = mountComponent(SettingsServiceOffAlert, { messages: { settings: en }, props: { service: 'usenet', enabled: false } })
    expect(await axeViolations(notice.container)).toBe('')
    const link = mountComponent(SettingsCrossLink, { messages: { settings: en }, props: { anchor: 'backup.full' } })
    expect(await axeViolations(link.container)).toBe('')
  })

  it('says nothing while the service runs', () => {
    const { container } = mountComponent(SettingsServiceOffAlert, { messages: { settings: en }, props: { service: 'torrent', enabled: true } })
    expect(container.querySelector('[data-service-off]')).toBeNull()
  })
})
