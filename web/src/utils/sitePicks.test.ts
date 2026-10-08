import { describe, expect, it } from 'vitest'

import type { CollectorPickEntry } from '@/api/types'

import { ALL, NO_EPISODE, attributeValues, noFilters, passes, sameEntries, seasonGroups, selectable } from './sitePicks'

function entry(index: number, attributes: Record<string, string>, state = 'pending'): CollectorPickEntry {
  return { index, label: `Show.${index}`, attributes, state, code: null, links: 0 }
}

const entries = [
  entry(0, { season: '1', episode: '7', resolution: 'SD', language: 'GERMAN' }),
  entry(1, { season: '1', episode: '7', resolution: '720p', language: 'GERMAN' }),
  entry(2, { season: '10', episode: '1', resolution: '1080p', language: 'ENGLISH' }),
  entry(3, { season: '1', resolution: '720p', language: 'GERMAN' }),
  entry(4, { season: '1', episode: '2', resolution: '720p', language: 'GERMAN' }, 'done'),
  entry(5, { season: '2', episode: '1', resolution: '2160p', language: 'GERMAN' }),
  entry(6, { resolution: '720p' })
]

describe('the choice before resolving', () => {
  it('groups by season in natural order, episodes in order and a season pack after them', () => {
    const groups = seasonGroups(entries)
    expect(groups.map(group => group.season)).toEqual(['1', '2', '10', null])
    expect(groups[0]?.entries.map(item => item.index)).toEqual([4, 0, 1, 3])
  })

  it('offers each filter only the values the page has, a season pack as its own choice', () => {
    expect(attributeValues(entries, 'season')).toEqual(['1', '2', '10'])
    expect(attributeValues(entries, 'resolution')).toEqual(['720p', '1080p', '2160p', 'SD'])
    expect(attributeValues(entries, 'episode')).toEqual(['1', '2', '7', NO_EPISODE])
  })

  it('filters by every attribute that is set, and finds the season packs', () => {
    const filters = { ...noFilters(), season: '1', resolution: '720p' }
    expect(entries.filter(item => passes(item, filters)).map(item => item.index)).toEqual([1, 3, 4])
    const packs = { ...noFilters(), episode: NO_EPISODE }
    expect(entries.filter(item => passes(item, packs)).map(item => item.index)).toEqual([3, 6])
    expect(entries.every(item => passes(item, { ...noFilters(), language: ALL }))).toBe(true)
  })

  it('lets only what has not been fetched, or failed, be picked', () => {
    expect(selectable(entries[0]!)).toBe(true)
    expect(selectable(entries[4]!)).toBe(false)
    expect(selectable(entry(9, {}, 'failed'))).toBe(true)
    expect(selectable(entry(9, {}, 'captcha'))).toBe(false)
  })

  it('finds the chosen releases again in a list read anew, by name (RD-1190-17)', () => {
    const fresh = [entry(0, {}), { ...entry(1, {}), label: 'Show.4' }, { ...entry(2, {}), label: 'Show.1' }]
    // Show.1 and Show.4 moved; Show.6 is no longer listed.
    expect(sameEntries(entries, fresh, [1, 4, 6])).toEqual([1, 2])
    const unnamed = [{ ...entry(0, {}), label: null }, { ...entry(1, {}), label: null }]
    expect(sameEntries(unnamed, unnamed, [1])).toEqual([1])
  })
})
