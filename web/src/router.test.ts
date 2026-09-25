/**
 * The addresses the settings and the remote jobs answer to (RD-110-29).
 *
 * `/settings` is the overview, the old `?tab=` links and every pre-rubric page address still
 * land on a page, and an address naming no page lands on the overview. The views are stubbed:
 * the router loads a matched view lazily on navigation, and what is under test is the route
 * table, not what the views render.
 */
import { describe, expect, it, vi } from 'vitest'

const view = (name: string) => ({ default: { name, template: '<div />' } })
vi.mock('./views/DownloadsView.vue', () => view('DownloadsView'))
vi.mock('./views/LinkGrabberView.vue', () => view('LinkGrabberView'))
vi.mock('./views/StreamsView.vue', () => view('StreamsView'))
vi.mock('./views/SubscriptionsView.vue', () => view('SubscriptionsView'))
vi.mock('./views/RemoteJobsView.vue', () => view('RemoteJobsView'))
vi.mock('./views/AutomationView.vue', () => view('AutomationView'))
vi.mock('./views/StatsView.vue', () => view('StatsView'))
vi.mock('./views/LogsView.vue', () => view('LogsView'))
vi.mock('./views/SettingsOverview.vue', () => view('SettingsOverview'))
vi.mock('./views/SettingsView.vue', () => view('SettingsView'))

import { router } from './router'

async function go(path: string): Promise<{ path: string, name: unknown, section: unknown }> {
  await router.push(path)
  const route = router.currentRoute.value
  return { path: route.path, name: route.name, section: route.params.section }
}

describe('the settings addresses', () => {
  it('renders the overview at /settings instead of redirecting to a page', async () => {
    expect(await go('/settings')).toEqual({ path: '/settings', name: 'settings-overview', section: undefined })
  })

  it('still turns the old ?tab= form into the page address', async () => {
    expect(await go('/settings?tab=usenet')).toEqual({ path: '/settings/usenet', name: 'settings', section: 'usenet' })
  })

  it('keeps every page address from before the rubrics', async () => {
    for (const section of ['system', 'network', 'media', 'bandwidth', 'accounts']) {
      expect(await go(`/settings/${section}`)).toEqual({ path: `/settings/${section}`, name: 'settings', section })
    }
  })

  it('reaches the pages that are new with the rubrics', async () => {
    for (const section of ['hotfolders', 'unattended', 'captcha', 'torrent', 'transfers', 'tools']) {
      expect(await go(`/settings/${section}`)).toEqual({ path: `/settings/${section}`, name: 'settings', section })
    }
  })

  it('sends an address that names no page to the overview', async () => {
    expect(await go('/settings/nonsense')).toEqual({ path: '/settings', name: 'settings-overview', section: undefined })
  })
})

describe('the remote jobs address', () => {
  it('is a route of its own beside the subscriptions', async () => {
    expect(await go('/remote-jobs')).toEqual({ path: '/remote-jobs', name: 'remote-jobs', section: undefined })
  })
})
