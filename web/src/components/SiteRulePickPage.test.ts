import { fireEvent, within } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { CollectorPick, CollectorPickEntry } from '@/api/types'
import linkgrabber from '@/locales/en/linkgrabber.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

import SiteRulePickPage from './SiteRulePickPage.vue'

function entry(index: number, attributes: Record<string, string>, state = 'pending', code: string | null = null): CollectorPickEntry {
  return { index, label: `The.Show.${index}`, attributes, state, code, links: state === 'done' ? 2 : 0 }
}

function page(entries: CollectorPickEntry[], extra: Partial<CollectorPick> = {}): CollectorPick {
  return {
    id: 'p1',
    rule: 'serienjunkies.org',
    rule_id: 'serienjunkies',
    address: 'https://serienjunkies.org/serie/the-show/',
    package_name: 'The Show',
    created_at: '2026-10-07T20:00:00Z',
    running: false,
    total: 0,
    finished: 0,
    waiting_for_captcha: false,
    entries,
    ...extra
  }
}

const releases = [
  entry(0, { season: '1', episode: '7', resolution: 'SD', language: 'GERMAN', hoster: 'ddownload' }),
  entry(1, { season: '1', episode: '7', resolution: '720p', language: 'GERMAN', hoster: 'ddownload' }),
  entry(2, { season: '1', resolution: '720p', language: 'GERMAN', hoster: 'ddownload' }),
  entry(3, { season: '2', episode: '1', resolution: '720p', language: 'GERMAN', hoster: 'ddownload' }),
  entry(4, { season: '2', episode: '2', resolution: '720p', language: 'GERMAN', hoster: 'ddownload' }, 'done')
]

function mount(current: CollectorPick) {
  return mountComponent(SiteRulePickPage, { messages: { linkgrabber, server }, props: { page: current, busy: false } })
}

describe('SiteRulePickPage', () => {
  it('groups the releases by season and fetches only what was picked', async () => {
    const { getByRole, getByTestId, emitted } = mount(page(releases))
    const season1 = getByRole('list', { name: 'Season 1' })
    expect(within(season1).getAllByRole('listitem')).toHaveLength(3)
    expect(within(season1).getByText('Season pack')).toBeTruthy()
    const fetch = getByTestId('pick-fetch')
    expect((fetch as HTMLButtonElement).disabled).toBe(true)

    // The whole of season 2 is one tick; the release already done is not taken along.
    await fireEvent.click(getByRole('checkbox', { name: 'Season 2' }))
    await fireEvent.click(getByRole('checkbox', { name: 'The.Show.1' }))
    expect(fetch.textContent).toContain('Fetch links of 2 releases')
    await fireEvent.click(fetch)
    expect(emitted('resolve')).toEqual([[[1, 3]]])
  })

  it('narrows the list with the quick filters and picks what they show', async () => {
    const { getByRole, getByTestId, emitted, queryByRole } = mount(page(releases))
    await fireEvent.update(getByRole('combobox', { name: 'Filter by resolution' }), '720p')
    await fireEvent.update(getByRole('combobox', { name: 'Filter by episode' }), 'pack')
    expect(queryByRole('checkbox', { name: 'The.Show.0' })).toBeNull()
    await fireEvent.click(getByRole('checkbox', { name: 'Pick the shown release' }))
    await fireEvent.click(getByTestId('pick-fetch'))
    expect(emitted('resolve')).toEqual([[[2]]])
  })

  it('counts the round and says when a captcha waits for a person', () => {
    const { getByTestId, getByRole } = mount(page([
      entry(0, { season: '1', episode: '1' }, 'done'),
      entry(1, { season: '1', episode: '2' }, 'captcha'),
      entry(2, { season: '1', episode: '3' }, 'pending', 'site_rules.captcha_failed')
    ], { running: true, total: 2, finished: 1, waiting_for_captcha: true }))
    expect(getByTestId('pick-progress').textContent).toContain('1 of 2')
    expect(getByTestId('pick-captcha').textContent).toContain('Waiting for captcha')
    expect((getByRole('checkbox', { name: 'The.Show.1' }) as HTMLInputElement).disabled).toBe(true)
    // An unanswered captcha leaves the release open to another try, and says why.
    const again = getByRole('checkbox', { name: 'The.Show.2' }) as HTMLInputElement
    expect(again.disabled).toBe(false)
    expect(getByRole('button', { name: 'Stop' })).toBeTruthy()
  })
})
