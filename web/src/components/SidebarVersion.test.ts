/**
 * The version at the foot of the sidebar (RD-1250-06): always there, the full form while the
 * sidebar is open and the short one on the rail, and a link to *About* in both. jsdom lays
 * nothing out, so the widths are the live check's; what is pinned here is the structure.
 */
import { screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })) },
  responseError: vi.fn()
}))

import { api } from '@/api/client'
import { BUILD_VERSION, checkServiceVersion, resetServiceVersionForTests } from '@/composables/serviceVersion'
import deNav from '@/locales/de/nav.json'
import enNav from '@/locales/en/nav.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SidebarVersion from './SidebarVersion.vue'

function mount(collapsed: boolean, locale: 'en' | 'de' = 'en') {
  return mountComponent(SidebarVersion, {
    messages: { nav: locale === 'de' ? deNav : enNav },
    locale,
    props: { collapsed }
  })
}

async function serviceAnswers(version: string): Promise<void> {
  vi.mocked(api.GET).mockResolvedValueOnce({ data: { version } } as never)
  await checkServiceVersion()
}

const link = () => screen.getByTestId('sidebar-version')

beforeEach(() => {
  resetServiceVersionForTests()
  vi.mocked(api.GET).mockReset()
})

describe('SidebarVersion', () => {
  it('names the build version until the service has answered, and leads to About', () => {
    mount(false)

    expect(link().getAttribute('href')).toBe('/settings/about')
    expect(link().textContent).toContain(`v${BUILD_VERSION}`)
  })

  it('shows the full version the service runs while the sidebar is open', async () => {
    await serviceAnswers('9.8.7')
    mount(false)

    expect(link().getAttribute('data-collapsed')).toBe('false')
    expect(link().querySelector('[aria-hidden]')).toBeNull()
    expect(link().textContent?.replace(/\s+/g, ' ').trim()).toBe('v9.8.7 – About rDownloader')
  })

  it('shows the short form on the rail and keeps the full one in the name and the tooltip', async () => {
    await serviceAnswers('9.8.7')
    mount(true)

    expect(link().getAttribute('data-collapsed')).toBe('true')
    expect(link().classList).toContain('justify-center')
    const visible = link().querySelector('[aria-hidden="true"]')
    expect(visible?.textContent).toBe('9.8')
    // The name is what is not hidden from assistive technology; the visible "9.8" is part of
    // it (WCAG 2.5.3).
    const name = link().querySelector('.visually-hidden')?.textContent?.trim() ?? ''
    expect(name).toBe('v9.8.7 – About rDownloader')
    expect(name).toContain('9.8')
    expect(link().closest('[text]')?.getAttribute('text')).toBe('Version 9.8.7 – About rDownloader')
  })

  it('speaks the reader\'s language', async () => {
    await serviceAnswers('9.8.7')
    mount(false, 'de')

    expect(link().textContent?.replace(/\s+/g, ' ').trim()).toBe('v9.8.7 – Über rDownloader')
  })

  it('renders without an axe violation, open and on the rail', async () => {
    await serviceAnswers('9.8.7')
    const open = mount(false)
    expect(await axeViolations(open.container)).toBe('')
    open.unmount()

    const rail = mount(true)
    expect(await axeViolations(rail.container)).toBe('')
  })
})
