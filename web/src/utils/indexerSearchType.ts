import type { IndexerCaps } from '@/api/types'

/**
 * The typed searches of the LinkGrabber's indexer search (RD-1100-03): `t=tvsearch` with season,
 * episode and the series' ids, `t=movie` with the film's. The person types the ids in; nothing
 * here looks one up.
 *
 * What is offered follows the indexers' own `t=caps` answers: a type only where an indexer
 * answers it, an id only where one of them takes it. The server checks the same rules again
 * (`crates/rd-api-intake/src/indexer_search/typed.rs`).
 */

/** `IndexerSearchType` in `crates/rd-subscription/src/query.rs`. */
type SearchType = 'search' | 'tv' | 'movie'
export const SEARCH_TYPES: readonly SearchType[] = ['search', 'tv', 'movie']

/** An id field by its wire name. */
export type IdField = 'season' | 'ep' | 'tvdbid' | 'tvmazeid' | 'imdbid' | 'tmdbid'

/** The element `t=caps` lists each type under. */
const CAPS_NAME: Record<SearchType, string> = { search: 'search', tv: 'tv-search', movie: 'movie-search' }

/** `IndexerSearchType::params` there: the fields each type may send, in the form's order. */
const TYPE_FIELDS: Record<SearchType, readonly IdField[]> = {
  search: [],
  tv: ['season', 'ep', 'tvdbid', 'tvmazeid', 'imdbid'],
  movie: ['imdbid', 'tmdbid']
}

/** The typed part of the search form, as the fields hold it. */
export interface TypedFields {
  type: SearchType
  ids: Record<IdField, string>
}

export function emptyTypedFields(): TypedFields {
  return { type: 'search', ids: { season: '', ep: '', tvdbid: '', tvmazeid: '', imdbid: '', tmdbid: '' } }
}

/** What an indexer's caps are while unknown (`undefined`), after a failed test (`null`), or known. */
type KnownCaps = IndexerCaps | null | undefined

/** Whether the indexer answers the type. Every indexer answers the plain search. */
export function answers(caps: KnownCaps, type: SearchType): boolean {
  if (type === 'search') return true
  return Boolean(caps?.searching.some(entry => entry.toLowerCase() === CAPS_NAME[type]))
}

/**
 * Whether the type can be offered for these indexers: until every answer is in it is (the caps
 * are asked when the menu opens), afterwards only when one of them answers it.
 */
export function offered(capsList: readonly KnownCaps[], type: SearchType): boolean {
  if (type === 'search' || capsList.some(caps => caps === undefined)) return true
  return capsList.some(caps => answers(caps, type))
}

/**
 * The id fields the type offers for these indexers: those one of the indexers answering it takes,
 * or all of the type's when one of them does not list its parameters, or has not answered yet.
 */
export function offeredFields(type: SearchType, capsList: readonly KnownCaps[]): IdField[] {
  const answering = capsList.filter(caps => caps === undefined || answers(caps, type))
  const fields = TYPE_FIELDS[type]
  const listed = answering.map(caps => caps?.supported_params?.[CAPS_NAME[type]])
  if (listed.some(params => !params)) return [...fields]
  return fields.filter(field => listed.some(params => params?.includes(field)))
}

const IMDB = /^(?:tt)?(\d{7,10})$/i

/** A field's value as a whole number of at least `minimum`, or `null`. */
function whole(value: string, minimum: number): number | null {
  const trimmed = value.trim()
  if (!/^\d+$/.test(trimmed)) return null
  const number = Number(trimmed)
  return Number.isSafeInteger(number) && number >= minimum ? number : null
}

/** A problem as the server would name it, so the same translation says it. */
interface TypedProblem {
  code: 'indexer.episode_without_season' | 'indexer.search_id_invalid'
  params: Record<string, string>
}

/** What is wrong with the typed fields the type sends, or `null`. */
export function typedProblem(fields: TypedFields, sent: readonly IdField[]): TypedProblem | null {
  const filled = sent.filter(field => fields.ids[field].trim() !== '')
  for (const field of filled) {
    const value = fields.ids[field]
    const valid = field === 'imdbid' ? IMDB.test(value.trim()) : whole(value, field === 'season' || field === 'ep' ? 0 : 1) !== null
    if (!valid) return { code: 'indexer.search_id_invalid', params: { field } }
  }
  if (filled.includes('ep') && !filled.includes('season')) return { code: 'indexer.episode_without_season', params: {} }
  return null
}

/** The request body's name for each field. */
const BODY_NAME: Record<IdField, string> = {
  season: 'season', ep: 'episode', tvdbid: 'tvdb_id', tvmazeid: 'tvmaze_id', imdbid: 'imdb_id', tmdbid: 'tmdb_id'
}

/**
 * The typed part of a search request: nothing for the plain search, so its body stays the one it
 * always was; otherwise the type and every filled field it sends. Call after `typedProblem`.
 */
export function typedBody(fields: TypedFields, sent: readonly IdField[]): Record<string, string | number> {
  if (fields.type === 'search') return {}
  const body: Record<string, string | number> = { search_type: fields.type }
  for (const field of sent) {
    const value = fields.ids[field].trim()
    if (!value) continue
    body[BODY_NAME[field]] = field === 'imdbid' ? value : Number(value)
  }
  return body
}
