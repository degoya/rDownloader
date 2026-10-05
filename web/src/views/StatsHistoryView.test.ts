/**
 * Statistics and history as two tabs of one page (RD-1101-05): the tab lives in the address, so
 * a reload, a bookmark and the back button land on it, and only the shown tab is mounted. The
 * tabs' own content is tested beside it (`StatsTab.test.ts`, `HistoryTab.test.ts`) and stubbed
 * here; the redirect from `/history` is the route table's (`router.test.ts`).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import nav from '@/locales/en/nav.json'
import { mountComponent } from '@/test/mount'

vi.mock('@/components/stats/StatsTab.vue', () => ({ default: { name: 'StatsTab', template: '<p data-testid="stats-tab">statistics</p>' } }))
vi.mock('@/components/history/HistoryTab.vue', () => ({ default: { name: 'HistoryTab', template: '<p data-testid="history-tab">history</p>' } }))

const { default: StatsHistoryView } = await import('./StatsHistoryView.vue')

async function mountAt(address: string): Promise<Router> {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/stats', component: StatsHistoryView }] })
  await router.push(address)
  await router.isReady()
  mountComponent(StatsHistoryView, {
    messages: { nav },
    plugins: [router],
    stubs: { UDashboardNavbar: { props: ['title'], template: '<header><h1>{{ title }}</h1><slot name="leading" /></header>' } }
  })
  return router
}

function selected(): string | undefined {
  return screen.getAllByRole('tab').find(tab => tab.getAttribute('aria-selected') === 'true')?.textContent ?? undefined
}

describe('StatsHistoryView', () => {
  it('names the page for both and opens on the statistics, the plain address', async () => {
    await mountAt('/stats')
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe(nav.stats_history)
    expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([nav.stats, nav.history])
    expect(await screen.findByTestId('stats-tab')).toBeTruthy()
    expect(screen.queryByTestId('history-tab')).toBeNull()
    expect(selected()).toBe(nav.stats)
  })

  it('opens the tab the address names', async () => {
    await mountAt('/stats?tab=history')
    expect(await screen.findByTestId('history-tab')).toBeTruthy()
    expect(screen.queryByTestId('stats-tab')).toBeNull()
    expect(selected()).toBe(nav.history)
  })

  it('shows the statistics for a tab the page does not have', async () => {
    await mountAt('/stats?tab=nonsense')
    expect(await screen.findByTestId('stats-tab')).toBeTruthy()
  })

  it('pushes the tab into the address, keeps the rest of it, and walks back with the browser', async () => {
    const router = await mountAt('/stats?range=week')
    await fireEvent.click(screen.getByRole('tab', { name: nav.history }))
    await vi.waitFor(() => expect(router.currentRoute.value.fullPath).toBe('/stats?range=week&tab=history'))
    expect(await screen.findByTestId('history-tab')).toBeTruthy()
    expect(screen.queryByTestId('stats-tab')).toBeNull()

    await fireEvent.click(screen.getByRole('tab', { name: nav.stats }))
    await vi.waitFor(() => expect(router.currentRoute.value.fullPath).toBe('/stats?range=week'))
    expect(await screen.findByTestId('stats-tab')).toBeTruthy()

    router.back()
    await vi.waitFor(() => expect(router.currentRoute.value.query.tab).toBe('history'))
    expect(await screen.findByTestId('history-tab')).toBeTruthy()
  })
})
