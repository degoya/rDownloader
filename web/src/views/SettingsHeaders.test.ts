/**
 * Every settings page opens the same way (RD-110-29): one page-level `SectionHeader` — an `h2`
 * with an eyebrow above and a description under it — and nothing else at that level.
 *
 * Measured on 2026-09-22 before this test existed: Desktop client had a bare paragraph, API &
 * MCP a card heading without a description, Security no header at all and Accounts a header
 * without a description. Each page is mounted through `SettingsView` at its own address, with
 * the service answering nothing, so what is counted is the page as a person reaches it.
 */
import { render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { i18n, SUPPORTED_LOCALES } from '@/i18n'
import { SETTINGS_SECTIONS } from '@/settingsSections'
import { uiStubs } from '@/test/mount'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    POST: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    PUT: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    PATCH: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    DELETE: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } }))
  },
  responseError: () => 'The service did not answer',
  resultMessage: () => ''
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }), overlays: [] })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))

import SettingsView from './SettingsView.vue'

/** The navbar with its title drawn, so the name it gives the page can be read. */
const UDashboardNavbar = { props: ['title'], template: '<header data-testid="navbar"><h1>{{ title }}</h1><slot /></header>' }

async function mountPage(section: string, locale = 'en'): Promise<HTMLElement> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: SettingsView }]
  })
  await router.push(`/settings/${section}`)
  await router.isReady()
  setActivePinia(createPinia())
  i18n.global.locale.value = locale as typeof i18n.global.locale.value
  const { container } = render(SettingsView, {
    global: { plugins: [router, i18n], stubs: { ...uiStubs, UDashboardNavbar } as never }
  })
  return container as HTMLElement
}

describe('the settings page headers', () => {
  it.each(SETTINGS_SECTIONS.map(section => section.value))('%s opens with exactly one page header, with a description', async (section) => {
    const container = await mountPage(section)
    const headings = container.querySelectorAll('h2')
    expect(headings, `${section}: page-level headings`).toHaveLength(1)
    const heading = headings[0] as HTMLElement
    expect(heading.textContent?.trim(), `${section}: title`).not.toBe('')

    const eyebrow = heading.previousElementSibling
    expect(eyebrow?.classList.contains('eyebrow'), `${section}: eyebrow above the title`).toBe(true)
    expect(eyebrow?.textContent?.trim()).not.toBe('')

    const description = heading.nextElementSibling
    expect(description?.tagName, `${section}: description under the title`).toBe('P')
    expect(description?.textContent?.trim()).not.toBe('')
    expect(description?.textContent).not.toMatch(/^[a-z_]+(\.[a-z_]+)+$/)
  })

  it('shows the title of the page the shared table names', async () => {
    for (const section of SETTINGS_SECTIONS) {
      const container = await mountPage(section.value)
      expect(container.querySelector('h2')?.textContent?.trim(), section.value).toBe(i18n.global.t(section.titleKey))
    }
  })

  // The navbar said "Settings" on all twenty-four pages; it names the page now, with the label
  // of the sidebar entry beside it, in every language (RD-120-53).
  it('names the page in the navbar, as the sidebar entry does, in all four languages', async () => {
    for (const locale of SUPPORTED_LOCALES) {
      for (const section of SETTINGS_SECTIONS) {
        const container = await mountPage(section.value, locale)
        const title = container.querySelector('[data-testid=navbar] h1')?.textContent?.trim()
        expect(title, `${locale}/${section.value}`).toBe(i18n.global.t(section.labelKey))
        expect(title, `${locale}/${section.value}`).not.toBe(i18n.global.t('settings.title'))
      }
    }
    i18n.global.locale.value = 'en'
  })
})
