import { describe, expect, it } from 'vitest'

import type { LinkCandidate } from '@/api/types'
import { SUPPORTED_LOCALES, i18n } from '@/i18n'
import { SORT_OPTIONS, hosterOf, sortCandidates, sortCollectorEntries } from './collectorSort'

function candidate(id: string, url: string, position: number, size?: string): LinkCandidate {
  return { id, batch_id: 'b', url, state: 'online', position, size: size ?? null } as LinkCandidate
}

describe('collectorSort', () => {
  const items = [
    candidate('2', 'https://www.rapidgator.net/file/b.part2.rar', 2, '200'),
    candidate('1', 'https://ddownload.com/x/b.part1.rar', 1, '100'),
    candidate('3', 'https://1fichier.com/?a.mkv', 3)
  ]

  it('keeps manual order by position', () => {
    expect(sortCandidates(items, 'manual', false).map(item => item.id)).toEqual(['1', '2', '3'])
  })

  it('sorts by name numerically and by hoster', () => {
    expect(sortCandidates(items, 'name', false).map(item => item.id)).toEqual(['3', '1', '2'])
    expect(sortCandidates(items, 'hoster', false).map(item => item.id)).toEqual(['3', '1', '2'])
    expect(hosterOf(items[0]!)).toBe('rapidgator.net')
    expect(hosterOf({ ...candidate('4', 'file:///tmp/test.torrent', 4), provider: 'torrent' })).toBe('torrent')
  })

  it('sorts by size with unknown sizes first and supports descending', () => {
    expect(sortCandidates(items, 'size', false).map(item => item.id)).toEqual(['3', '1', '2'])
    expect(sortCandidates(items, 'size', true).map(item => item.id)).toEqual(['2', '1', '3'])
  })

  it('compares names with the active locale', () => {
    const previous = i18n.global.locale.value
    const names = [candidate('a', 'https://h.tld/z.mkv', 1), candidate('b', 'https://h.tld/ä.mkv', 2), candidate('c', 'https://h.tld/b.mkv', 3)]
    for (const locale of SUPPORTED_LOCALES) {
      i18n.global.locale.value = locale
      expect(sortCandidates(names, 'name', false).map(item => item.id)).toEqual(['b', 'c', 'a'])
    }
    i18n.global.locale.value = previous
  })

  it('has a translated label for every sort option in every locale', () => {
    for (const locale of SUPPORTED_LOCALES) {
      for (const option of SORT_OPTIONS) {
        expect(i18n.global.te(option.labelKey, locale)).toBe(true)
      }
    }
  })

  it('sorts by added via created_at with the id as tiebreaker', () => {
    const added = [
      { ...candidate('b', 'https://h.tld/1.mkv', 1), created_at: '2026-01-02T00:00:00Z' },
      { ...candidate('a', 'https://h.tld/2.mkv', 2), created_at: '2026-01-01T00:00:00Z' },
      { ...candidate('c', 'https://h.tld/3.mkv', 3), created_at: '2026-01-01T00:00:00Z' }
    ] as LinkCandidate[]
    expect(sortCandidates(added, 'added', false).map(item => item.id)).toEqual(['a', 'c', 'b'])
    expect(sortCandidates(added, 'added', true).map(item => item.id)).toEqual(['b', 'c', 'a'])
  })
})

describe('sortCollectorEntries', () => {
  function entry(name: string, createdAt: string, candidates: LinkCandidate[]) {
    return { createdAt, package: { name }, candidates }
  }
  const entries = [
    entry('beta', '2026-01-03T00:00:00Z', [candidate('1', 'https://rapidgator.net/a.part1.rar', 1, '300')]),
    entry('alpha 10', '2026-01-01T00:00:00Z', [candidate('2', 'https://ddownload.com/b.rar', 1, '100'), candidate('3', 'https://ddownload.com/c.rar', 2, '100')]),
    entry('alpha 2', '2026-01-02T00:00:00Z', [candidate('4', 'https://1fichier.com/?d.mkv', 1)])
  ]

  it('keeps manual order untouched', () => {
    expect(sortCollectorEntries(entries, 'manual', false).map(item => item.package.name)).toEqual(['beta', 'alpha 10', 'alpha 2'])
  })

  it('sorts packages by name numerically', () => {
    expect(sortCollectorEntries(entries, 'name', false).map(item => item.package.name)).toEqual(['alpha 2', 'alpha 10', 'beta'])
    expect(sortCollectorEntries(entries, 'name', true).map(item => item.package.name)).toEqual(['beta', 'alpha 10', 'alpha 2'])
  })

  it('sorts packages by the first candidate hoster', () => {
    expect(sortCollectorEntries(entries, 'hoster', false).map(item => item.package.name)).toEqual(['alpha 2', 'alpha 10', 'beta'])
  })

  it('sorts packages by total size', () => {
    expect(sortCollectorEntries(entries, 'size', false).map(item => item.package.name)).toEqual(['alpha 2', 'alpha 10', 'beta'])
    expect(sortCollectorEntries(entries, 'size', true).map(item => item.package.name)).toEqual(['beta', 'alpha 10', 'alpha 2'])
  })

  it('sorts packages by creation time', () => {
    expect(sortCollectorEntries(entries, 'added', false).map(item => item.package.name)).toEqual(['alpha 10', 'alpha 2', 'beta'])
  })
})
