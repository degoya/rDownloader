import { describe, expect, it } from 'vitest'

import type { IndexerCaps } from '@/api/types'

import { answers, emptyTypedFields, offered, offeredFields, typedBody, typedProblem, type TypedFields } from './indexerSearchType'

const JACKETT: IndexerCaps = {
  categories: [],
  searching: ['search', 'tv-search', 'movie-search'],
  supported_params: { 'search': ['q'], 'tv-search': ['q', 'season', 'ep', 'tvdbid'], 'movie-search': ['q', 'imdbid'] }
}
const PLAIN: IndexerCaps = { categories: [], searching: ['search'] }
/** An indexer that answers TV searches without saying which ids it takes. */
const SILENT: IndexerCaps = { categories: [], searching: ['search', 'tv-search'] }

function fields(type: TypedFields['type'], ids: Partial<TypedFields['ids']> = {}): TypedFields {
  const empty = emptyTypedFields()
  return { type, ids: { ...empty.ids, ...ids } }
}

describe('what the indexers offer', () => {
  it('offers a type only where an indexer answers it, once every answer is in', () => {
    expect(answers(PLAIN, 'tv')).toBe(false)
    expect(answers(JACKETT, 'tv')).toBe(true)
    expect(answers(null, 'search')).toBe(true)
    expect(offered([PLAIN], 'movie')).toBe(false)
    expect(offered([PLAIN, JACKETT], 'movie')).toBe(true)
    // While an answer is still out, nothing is switched off yet.
    expect(offered([PLAIN, undefined], 'movie')).toBe(true)
    // A failed test answers the free search alone.
    expect(offered([null], 'tv')).toBe(false)
  })

  it('shows the ids an answering indexer takes, or all of them when one does not say', () => {
    expect(offeredFields('tv', [JACKETT, PLAIN])).toEqual(['season', 'ep', 'tvdbid'])
    expect(offeredFields('movie', [JACKETT])).toEqual(['imdbid'])
    expect(offeredFields('tv', [JACKETT, SILENT])).toEqual(['season', 'ep', 'tvdbid', 'tvmazeid', 'imdbid'])
    expect(offeredFields('search', [JACKETT])).toEqual([])
    // Before the answers are in every id is shown; after a failed test none is.
    expect(offeredFields('movie', [undefined])).toEqual(['imdbid', 'tmdbid'])
    expect(offeredFields('movie', [null])).toEqual([])
  })
})

describe('typedProblem', () => {
  it('names an id that is not one, and an episode without its season, in the server\'s codes', () => {
    expect(typedProblem(fields('tv', { season: '1', ep: '2', tvdbid: '81189' }), ['season', 'ep', 'tvdbid'])).toBeNull()
    expect(typedProblem(fields('tv', { tvdbid: 'abc' }), ['tvdbid'])).toEqual({ code: 'indexer.search_id_invalid', params: { field: 'tvdbid' } })
    expect(typedProblem(fields('tv', { tvdbid: '0' }), ['tvdbid'])?.code).toBe('indexer.search_id_invalid')
    expect(typedProblem(fields('tv', { ep: '3' }), ['season', 'ep'])).toEqual({ code: 'indexer.episode_without_season', params: {} })
    expect(typedProblem(fields('movie', { imdbid: 'tt0133093' }), ['imdbid'])).toBeNull()
    expect(typedProblem(fields('movie', { imdbid: 'nm0000206' }), ['imdbid'])?.code).toBe('indexer.search_id_invalid')
    // A field the type does not send is not checked, whatever it still holds.
    expect(typedProblem(fields('movie', { season: 'x' }), ['imdbid'])).toBeNull()
  })
})

describe('typedBody', () => {
  it('adds nothing to a free search, so its body stays the one it always was', () => {
    expect(typedBody(fields('search', { season: '1' }), [])).toEqual({})
  })

  it('sends the type and the filled ids it shows, numbers as numbers', () => {
    expect(typedBody(fields('tv', { season: '1', ep: '2', imdbid: ' tt0903747 ', tmdbid: '603' }), ['season', 'ep', 'tvdbid', 'imdbid']))
      .toEqual({ search_type: 'tv', season: 1, episode: 2, imdb_id: 'tt0903747' })
  })
})
