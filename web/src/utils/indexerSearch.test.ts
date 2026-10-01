import { describe, expect, it } from 'vitest'

import type { IndexerSearchHit } from '@/api/types'

import { ageInDays, hitKey, maxAgeDays, queryProblem, sortHits } from './indexerSearch'

function hit(title: string, extra: Partial<IndexerSearchHit> = {}): IndexerSearchHit {
  return {
    indexer_id: 'one',
    indexer_name: 'One',
    title,
    download: `https://indexer.test/getnzb/${title}?apikey=rdownloader-indexer-key`,
    passworded: false,
    ...extra
  }
}

describe('queryProblem', () => {
  it('lets an empty term through and asks for three characters otherwise', () => {
    expect(queryProblem('')).toBeNull()
    expect(queryProblem('   ')).toBeNull()
    expect(queryProblem('ab')).toBe('indexer.query_too_short')
    expect(queryProblem(' x ')).toBe('indexer.query_too_short')
    expect(queryProblem('abc')).toBeNull()
    // An exclusion is passed on as written and counts like any other character.
    expect(queryProblem('some show !cam')).toBeNull()
    expect(queryProblem('a'.repeat(201))).toBe('indexer.query_too_long')
  })
})

describe('maxAgeDays', () => {
  it('reads whole days in range and nothing else', () => {
    expect(maxAgeDays('')).toBeNull()
    expect(maxAgeDays(undefined)).toBeNull()
    expect(maxAgeDays('30')).toBe(30)
    expect(maxAgeDays(7)).toBe(7)
    expect(maxAgeDays('0')).toBeNull()
    expect(maxAgeDays('1.5')).toBeNull()
    expect(maxAgeDays('10001')).toBeNull()
  })
})

describe('ageInDays', () => {
  it('counts whole days and knows when there is no date', () => {
    const now = Date.parse('2026-10-01T12:00:00Z')
    expect(ageInDays('2026-09-29T10:00:00Z', now)).toBe(2)
    expect(ageInDays('2026-10-02T10:00:00Z', now)).toBe(0)
    expect(ageInDays(null, now)).toBeNull()
    expect(ageInDays('not a date', now)).toBeNull()
  })
})

describe('sortHits', () => {
  const hits = [
    hit('b', { size_bytes: 200, published_at: '2026-09-01T00:00:00Z', grabs: 3 }),
    hit('a', { size_bytes: null, published_at: '2026-09-20T00:00:00Z' }),
    hit('c', { size_bytes: 100, published_at: null, grabs: 9 })
  ]

  it('keeps the indexers’ order when nothing is chosen', () => {
    expect(sortHits(hits, null, false).map(entry => entry.title)).toEqual(['b', 'a', 'c'])
  })

  it('sorts by title and by size, with an unknown value last in both directions', () => {
    expect(sortHits(hits, 'title', false).map(entry => entry.title)).toEqual(['a', 'b', 'c'])
    expect(sortHits(hits, 'size', false).map(entry => entry.title)).toEqual(['c', 'b', 'a'])
    expect(sortHits(hits, 'size', true).map(entry => entry.title)).toEqual(['b', 'c', 'a'])
    expect(sortHits(hits, 'grabs', true).map(entry => entry.title)).toEqual(['c', 'b', 'a'])
  })

  it('puts the newest first by age ascending', () => {
    expect(sortHits(hits, 'age', false).map(entry => entry.title)).toEqual(['a', 'b', 'c'])
  })

  it('tells the same release from two indexers apart', () => {
    expect(hitKey(hit('a'))).not.toBe(hitKey(hit('a', { indexer_id: 'two' })))
  })
})
