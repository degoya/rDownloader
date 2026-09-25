import { describe, expect, it } from 'vitest'

import type { LinkCandidate } from '@/api/types'

import { EMPTY_PREFERENCE, facetValues, filterRows, hideHosters, matchesPreference, mirrorRows, type LinkRow, type MirrorGroup, type MirrorPreference } from './mirrorGroups'

/** Narrows a row to its group, failing the test rather than typing the assertion away. */
function groupOf(row: LinkRow | undefined): MirrorGroup {
  if (!row || row.kind !== 'group') throw new Error('expected a mirror group row')
  return row.group
}

function link(id: string, url: string, mirror?: LinkCandidate['mirror'], state: LinkCandidate['state'] = 'online'): LinkCandidate {
  return {
    id,
    batch_id: 'batch-1',
    url,
    state,
    file_name: `${id}.mkv`,
    created_at: '2026-09-21T10:00:00Z',
    priority: 'normal',
    position: 1,
    package_id: 'package-1',
    mirror
  } as LinkCandidate
}

function group(selected: boolean, quality: string | null, language: string | null, extra: Partial<NonNullable<LinkCandidate['mirror']>> = {}) {
  return {
    group: 'release',
    source: 'declared',
    selected,
    quality: quality ?? undefined,
    language: language ?? undefined,
    ...extra
  } as NonNullable<LinkCandidate['mirror']>
}

const preference = (partial: Partial<MirrorPreference>): MirrorPreference => ({ ...EMPTY_PREFERENCE, ...partial })

describe('mirrorRows', () => {
  // The whole point of the job: fifteen links to one episode are one decision, not fifteen.
  it('draws one row per group and keeps the lone links beside it', () => {
    const rows = mirrorRows([
      link('a', 'https://a.example/1', group(false, '720p', 'German')),
      link('b', 'https://b.example/2', group(true, '1080p', 'German')),
      link('c', 'https://c.example/3')
    ])
    expect(rows).toHaveLength(2)
    expect(rows[0]).toMatchObject({ kind: 'group' })
    expect(groupOf(rows[0]).members.map(member => member.id)).toEqual(['a', 'b'])
    // The chosen member is the one the server marked, not the first one on screen.
    expect(groupOf(rows[0]).chosen.id).toBe('b')
    expect(rows[1]).toMatchObject({ kind: 'link' })
  })

  // RD-101-06 holds for a group as it holds for a link: it stays recognisable and enqueueable.
  it('counts how many mirrors are still worth starting', () => {
    const rows = mirrorRows([
      link('a', 'https://a.example/1', group(true, null, null), 'offline'),
      link('b', 'https://b.example/2', group(false, null, null), 'offline')
    ])
    expect(groupOf(rows[0]).onlineCount).toBe(0)
  })

  it('reports a group somebody pinned, and only through its chosen member', () => {
    const rows = mirrorRows([
      link('a', 'https://a.example/1', group(true, null, null, { pinned: true })),
      link('b', 'https://b.example/2', group(false, null, null))
    ])
    expect(groupOf(rows[0]).pinned).toBe(true)
  })
})

describe('matchesPreference', () => {
  // Hiding a link because nobody could tell its quality would hide it for not being labelled,
  // and the labelling is exactly what the application refuses to invent.
  it('never rejects a candidate on a facet it says nothing about', () => {
    const plain = link('a', 'https://a.example/1')
    expect(matchesPreference(plain, preference({ quality: '1080p' }))).toBe(true)
    expect(matchesPreference(plain, preference({ hoster: 'b.example' }))).toBe(false)
  })

  it('combines the facets', () => {
    const mirror = link('a', 'https://a.example/1', group(true, '1080p', 'German'))
    expect(matchesPreference(mirror, preference({ quality: '1080p', language: 'German' }))).toBe(true)
    expect(matchesPreference(mirror, preference({ quality: '1080p', language: 'English' }))).toBe(false)
  })
})

describe('filterRows', () => {
  const rows = () => mirrorRows([
    link('a', 'https://a.example/1', group(true, '720p', 'German')),
    link('b', 'https://b.example/2', group(false, '1080p', 'German')),
    link('c', 'https://c.example/3', { group: 'other', source: 'name', selected: true } as NonNullable<LinkCandidate['mirror']>),
    link('d', 'https://d.example/4', { group: 'other', source: 'name', selected: false } as NonNullable<LinkCandidate['mirror']>)
  ])

  // Where there is a choice the preference has already made it, so the group stays whole.
  it('keeps a group that holds a matching mirror and drops one that holds none', () => {
    const kept = filterRows(rows(), preference({ quality: '1080p' }))
    expect(kept.map(row => row.kind === 'group' ? row.group.key : row.candidate.id)).toEqual(['release', 'other'])
    const narrow = filterRows(rows(), preference({ quality: '2160p' }))
    // `other` carries no quality at all, so it is not rejected for lacking one; `release` is.
    expect(narrow.map(row => row.kind === 'group' ? row.group.key : row.candidate.id)).toEqual(['other'])
  })

  it('passes everything through when nothing is preferred', () => {
    expect(filterRows(rows(), EMPTY_PREFERENCE)).toHaveLength(2)
  })

  it('never hides a group somebody pinned', () => {
    const pinned = mirrorRows([
      link('a', 'https://a.example/1', group(true, '720p', 'German', { pinned: true })),
      link('b', 'https://b.example/2', group(false, '720p', 'German'))
    ])
    expect(filterRows(pinned, preference({ quality: '1080p' }))).toHaveLength(1)
  })
})

describe('hideHosters (RD-130-21)', () => {
  const rows = () => mirrorRows([
    link('a', 'https://a.example/1', group(true, null, null)),
    link('b', 'https://b.example/2', group(false, null, null)),
    link('c', 'https://www.A.example/3'),
    link('d', 'https://d.example/4')
  ])
  const ids = (kept: LinkRow[]) => kept.map(row => row.kind === 'link' ? row.candidate.id : row.group.key)

  // A group stays while one member is at a shown hoster; its hidden member is the fallback.
  it('drops a lone link of a hidden hoster and keeps a group one shown member holds', () => {
    expect(ids(hideHosters(rows(), new Set(['a.example'])))).toEqual(['release', 'd'])
  })

  it('drops a group only when every member sits at a hidden hoster', () => {
    expect(ids(hideHosters(rows(), new Set(['a.example', 'b.example'])))).toEqual(['d'])
  })

  it('passes everything through when nothing is hidden', () => {
    expect(hideHosters(rows(), new Set())).toHaveLength(3)
  })
})

describe('facetValues', () => {
  it('offers only the values the list actually carries', () => {
    const candidates = [
      link('a', 'https://a.example/1', group(true, '1080p', 'German')),
      link('b', 'https://b.example/2', group(false, '720p', null)),
      link('c', 'https://c.example/3')
    ]
    expect(facetValues(candidates, 'quality')).toEqual(['1080p', '720p'])
    expect(facetValues(candidates, 'language')).toEqual(['German'])
    expect(facetValues(candidates, 'hoster')).toEqual(['a.example', 'b.example', 'c.example'])
  })
})
