/**
 * The settings entry page (RD-110-29): one card per page under its rubric, each carrying the
 * page's own title and description and leading to it.
 */
import { screen, within } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import plugins from '@/locales/en/plugins.json'
import routing from '@/locales/en/routing.json'
import settings from '@/locales/en/settings.json'
import siterules from '@/locales/en/siterules.json'
import usenet from '@/locales/en/usenet.json'
import { SETTINGS_SECTION_GROUPS, SETTINGS_SECTIONS } from '@/settingsSections'
import { mountComponent } from '@/test/mount'

import SettingsOverview from './SettingsOverview.vue'

function mount() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/settings', component: SettingsOverview },
      { path: '/settings/:section', component: { template: '<div />' } }
    ]
  })
  return { router, ...mountComponent(SettingsOverview, { messages: { plugins, routing, settings, siterules, usenet }, plugins: [router] }) }
}

/** Walks a dotted key through the English catalogues the page reads. */
function text(key: string): string {
  const catalogues: Record<string, unknown> = { plugins, routing, settings, siterules, usenet }
  const value = key.split('.').reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], catalogues)
  expect(value, key).toBeTypeOf('string')
  return value as string
}

describe('SettingsOverview', () => {
  it('shows one section per rubric, in the sidebar order, headed by the rubric name', () => {
    mount()
    const groups = document.querySelectorAll('[data-settings-group]')
    expect(groups).toHaveLength(SETTINGS_SECTION_GROUPS.length)
    SETTINGS_SECTION_GROUPS.forEach((group, index) => {
      const heading = within(groups[index] as HTMLElement).getByRole('heading', { level: 3 })
      expect(heading.textContent?.trim()).toBe(text(group.labelKey))
      expect(groups[index]?.getAttribute('aria-labelledby')).toBe(heading.id)
    })
  })

  it('shows one card per page, with the page\'s title and description, under its rubric', () => {
    mount()
    const groups = [...document.querySelectorAll('[data-settings-group]')]
    for (const group of SETTINGS_SECTION_GROUPS) {
      const section = groups[SETTINGS_SECTION_GROUPS.indexOf(group)] as HTMLElement
      const cards = section.querySelectorAll('[data-settings-card]')
      expect(cards).toHaveLength(group.sections.length)
      group.sections.forEach((page, index) => {
        const card = cards[index] as HTMLElement
        expect(card.textContent).toContain(text(page.titleKey))
        expect(card.textContent).toContain(text(page.descriptionKey))
      })
    }
    expect(document.querySelectorAll('[data-settings-card]')).toHaveLength(SETTINGS_SECTIONS.length)
  })

  it('makes every card a link to its page, so the keyboard reaches it', async () => {
    const { router } = mount()
    await router.isReady()
    for (const page of SETTINGS_SECTIONS) {
      const link = screen.getByRole('link', { name: new RegExp(`^${text(page.titleKey).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`) })
      expect(link.getAttribute('href')).toBe(`/settings/${page.value}`)
      expect(link.tabIndex).toBe(0)
    }
  })

  it('opens the same page header as every settings page', () => {
    mount()
    const headings = screen.getAllByRole('heading', { level: 2 })
    expect(headings).toHaveLength(1)
    expect(headings[0]?.textContent?.trim()).toBe(settings.overview.title)
    expect(screen.getByText(settings.overview.description)).toBeTruthy()
  })
})
