import type { CollectorPickEntry } from '@/api/types'

/**
 * The choice before resolving (RD-1170-03): a series page's releases grouped by season, the
 * quick filters over them, and which of them can still be picked.
 *
 * Pure functions, so the grouping and the filters are tested without a component.
 */

/** A filter that is off; an empty string is not a legal select value (Reka UI throws on it). */
export const ALL = 'all'
/** The episode filter's value for an entry without an episode: a season pack. */
export const NO_EPISODE = 'pack'

/** The attributes the quick filters work on, in the order they are shown. */
export const FILTERED = ['season', 'episode', 'resolution', 'language'] as const
export type FilteredAttribute = typeof FILTERED[number]
export type PickFilters = Record<FilteredAttribute, string>

export function noFilters(): PickFilters {
  return { season: ALL, episode: ALL, resolution: ALL, language: ALL }
}

/** Numbers by their value, everything else by its text: `2` before `10`, `720p` before `1080p`. */
function compareValues(left: string, right: string): number {
  return left.localeCompare(right, undefined, { numeric: true, sensitivity: 'base' })
}

/** The values one attribute takes among the entries, sorted; `NO_EPISODE` where an entry has none. */
export function attributeValues(entries: readonly CollectorPickEntry[], name: FilteredAttribute): string[] {
  const values = new Set<string>()
  for (const entry of entries) {
    const value = entry.attributes[name]
    if (value !== undefined) values.add(value)
    else if (name === 'episode') values.add(NO_EPISODE)
  }
  return [...values].sort((left, right) => {
    if (left === NO_EPISODE) return 1
    if (right === NO_EPISODE) return -1
    return compareValues(left, right)
  })
}

/** Whether an entry passes every filter that is on. */
export function passes(entry: CollectorPickEntry, filters: PickFilters): boolean {
  return FILTERED.every((name) => {
    const wanted = filters[name]
    if (wanted === ALL) return true
    const value = entry.attributes[name]
    if (name === 'episode' && wanted === NO_EPISODE) return value === undefined
    return value === wanted
  })
}

export interface SeasonGroup {
  /** The season, or `null` for entries the rule read none for. */
  season: string | null
  entries: CollectorPickEntry[]
}

/**
 * The entries by season, seasons in their natural order and the ones without a season last;
 * inside a season, episodes in order and a season pack after them, the page's order otherwise.
 */
export function seasonGroups(entries: readonly CollectorPickEntry[]): SeasonGroup[] {
  const groups = new Map<string | null, CollectorPickEntry[]>()
  for (const entry of entries) {
    const season = entry.attributes.season ?? null
    const group = groups.get(season)
    if (group) group.push(entry)
    else groups.set(season, [entry])
  }
  const episode = (entry: CollectorPickEntry): number => {
    const value = Number(entry.attributes.episode)
    return Number.isFinite(value) && entry.attributes.episode !== undefined ? value : Number.POSITIVE_INFINITY
  }
  return [...groups.entries()]
    .sort(([left], [right]) => {
      if (left === null) return 1
      if (right === null) return -1
      return compareValues(left, right)
    })
    .map(([season, members]) => ({
      season,
      entries: [...members].sort((left, right) => episode(left) - episode(right) || left.index - right.index)
    }))
}

/** Whether an entry can be picked: nothing of it fetched yet, or a refusal worth another try. */
export function selectable(entry: CollectorPickEntry): boolean {
  return entry.state === 'pending' || entry.state === 'failed'
}
