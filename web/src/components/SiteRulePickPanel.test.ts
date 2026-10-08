/**
 * The pick panel's drawer (RD-1190-17): it does not go down with the header when the last list
 * goes, it closes instead — the drawer used to be unmounted while open and left the page dimmed
 * and unclickable — and a page another intake listed opens it as a paste does.
 */
import { fireEvent, render, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import type { CollectorPick } from '@/api/types'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { useSitePicksStore } from '@/stores/sitePicks'
import { createTestI18n, mountComponent, uiStubs } from '@/test/mount'

import SiteRulePickPanel from './SiteRulePickPanel.vue'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'failed')
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

function page(id: string): CollectorPick {
  return {
    id,
    rule: 'serienjunkies.org',
    rule_id: 'serienjunkies',
    address: 'https://serienjunkies.org/serie/the-show/',
    package_name: 'The Show',
    created_at: '2026-10-08T20:00:00Z',
    running: false,
    total: 0,
    finished: 0,
    waiting_for_captcha: false,
    entries: [{ index: 0, label: 'The.Show.S01E01', attributes: {}, state: 'pending', code: null, links: 0 }]
  }
}

const stubs = {
  UDrawer: {
    props: ['open'],
    template: '<div data-testid="pick-drawer" :data-open="String(Boolean(open))"><slot v-if="open" name="body" /></div>'
  },
  SiteRulePickPage: { props: ['page'], template: '<div data-testid="pick-page">{{ page.id }}</div>' }
}

function mount() {
  return mountComponent(SiteRulePickPanel, { messages: { linkgrabber }, stubs })
}

/** Mounts again over the Pinia that is already active, as a view that opens later does. */
function mountAgain() {
  return render(SiteRulePickPanel, {
    global: { plugins: [createTestI18n({ linkgrabber })] as never[], stubs: { ...uiStubs, ...stubs } as never }
  })
}

async function settle(): Promise<void> {
  for (let round = 0; round < 4; round += 1) await nextTick()
}

describe('SiteRulePickPanel', () => {
  it('closes the drawer when the last list is gone instead of taking it down with the header', async () => {
    get.mockResolvedValue({ data: { pages: [page('p1')] } })
    mount()
    await settle()
    await fireEvent.click(screen.getByRole('button', { name: linkgrabber.picks.open }))
    expect(screen.getByTestId('pick-drawer').dataset.open).toBe('true')

    const picks = useSitePicksStore()
    picks.pages = []
    await settle()
    expect(screen.queryByTestId('pick-panel')).toBeNull()
    // Still there, and closed: not unmounted while open.
    expect(screen.getByTestId('pick-drawer').dataset.open).toBe('false')
    picks.stop()
  })

  it('opens on a page another intake listed while the LinkGrabber was not shown', async () => {
    get.mockResolvedValue({ data: { pages: [page('p1')] } })
    const rendered = mount()
    const picks = useSitePicksStore()
    rendered.unmount()
    // Announced with no panel on screen, then the LinkGrabber is opened.
    await picks.announced({ list: 'p1', entries: 1, rule: 'serienjunkies.org' })
    expect(picks.unseen).toBe(true)
    mountAgain()
    await settle()
    expect(screen.getByTestId('pick-drawer').dataset.open).toBe('true')
  })
})
