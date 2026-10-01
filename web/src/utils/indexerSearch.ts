import type { IndexerSearchHit } from '@/api/types'

/**
 * The interactive indexer search's rules on the client side (RD-180-19).
 *
 * The server checks every one of them again; they are here so a search the indexer would refuse
 * is never sent. Each search is a request the indexer counts against a daily limit, and
 * omgwtfnzbs caches an answer for ten minutes, so a refused one costs more than a moment.
 */

/** `MIN_INDEXER_QUERY_CHARS` in `crates/rd-core/src/indexer.rs`: shorter is Newznab error 201. */
export const MIN_QUERY_CHARS = 3
/** `MAX_INDEXER_QUERY_CHARS` there. */
export const MAX_QUERY_CHARS = 200
/** `MAX_INDEXER_AGE_DAYS` there. */
export const MAX_AGE_DAYS = 10_000
/** `MAX_SEARCH_LIMIT` in `crates/rd-subscription/src/search.rs`. */
export const MAX_LIMIT = 500
/** The page sizes offered; the server's own default is 100. */
export const LIMITS = [50, 100, 250, MAX_LIMIT] as const
export const DEFAULT_LIMIT = 100

/** The server's code for what is wrong with a search term, or `null` when it may be sent. */
export type QueryProblem = 'indexer.query_too_short' | 'indexer.query_too_long' | 'indexer.query_invalid'

/**
 * What is wrong with `query`: empty is fine (the newest releases), otherwise it needs three
 * characters. `!word` exclusions count as written — the indexer reads them, not we.
 */
export function queryProblem(query: string): QueryProblem | null {
  const term = query.trim()
  if (!term) return null
  const length = [...term].length
  if (length < MIN_QUERY_CHARS) return 'indexer.query_too_short'
  if (length > MAX_QUERY_CHARS) return 'indexer.query_too_long'
  if (/[\r\n\0]/.test(term)) return 'indexer.query_invalid'
  return null
}

/** The `maxage` a field holds: a whole number of days in range, or `null` for none or nonsense. */
export function maxAgeDays(value: string | number | null | undefined): number | null {
  if (value === null || value === undefined || String(value).trim() === '') return null
  const days = Number(value)
  return Number.isInteger(days) && days >= 1 && days <= MAX_AGE_DAYS ? days : null
}

/** The search fields of an indexer subscription's form (RD-180-20). */
export interface IndexerSearchFields {
  query: string
  maxAge: string | number
  hidePassworded: boolean
  /** `none` sends no `pred`; a select item cannot carry the empty string. */
  pretime: 'none' | '0' | '1' | '2'
}

/** No defined indexer chosen in the subscription form. */
export const NO_INDEXER = '__none__'

/** A hit's identity in a selection: the same release may come from two indexers. */
export function hitKey(hit: Pick<IndexerSearchHit, 'indexer_id' | 'download'>): string {
  return `${hit.indexer_id}\n${hit.download}`
}

export type HitSortKey = 'title' | 'size' | 'age' | 'category' | 'indexer' | 'grabs'

/** Whole days since `published`, never negative; `null` when the indexer gave no date. */
export function ageInDays(published: string | null | undefined, now: number = Date.now()): number | null {
  if (!published) return null
  const time = Date.parse(published)
  if (Number.isNaN(time)) return null
  return Math.max(0, Math.floor((now - time) / 86_400_000))
}

function sortValue(hit: IndexerSearchHit, key: HitSortKey): string | number | null {
  switch (key) {
    case 'title': return hit.title.toLocaleLowerCase()
    case 'size': return hit.size_bytes ?? null
    // Sorted by the date itself: "newest first" is ascending age.
    case 'age': return hit.published_at ? -Date.parse(hit.published_at) : null
    case 'category': return hit.category ?? null
    case 'indexer': return hit.indexer_name.toLocaleLowerCase()
    case 'grabs': return hit.grabs ?? null
  }
}

/**
 * The hits in the chosen order. A hit without the value sorts last in either direction —
 * an unknown size is not the smallest one — and equal values keep the indexers' own order.
 */
export function sortHits(hits: readonly IndexerSearchHit[], key: HitSortKey | null, descending: boolean): IndexerSearchHit[] {
  if (!key) return [...hits]
  const direction = descending ? -1 : 1
  return hits
    .map((hit, position) => ({ hit, position, value: sortValue(hit, key) }))
    .sort((left, right) => {
      if (left.value === null || right.value === null) {
        if (left.value === right.value) return left.position - right.position
        return left.value === null ? 1 : -1
      }
      if (left.value < right.value) return -direction
      if (left.value > right.value) return direction
      return left.position - right.position
    })
    .map(entry => entry.hit)
}
