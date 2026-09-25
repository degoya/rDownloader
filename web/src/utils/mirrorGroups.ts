/**
 * Mirror groups as the LinkGrabber draws them (RD-110-19).
 *
 * The server decides which member of a group is the chosen one — the standing preference and
 * a pin both live there, because the choice is what the queue will fetch. What lives here is
 * the reading of that answer: one row per group instead of one row per link, the facet values
 * a group can offer, and the filter that hides what no member can satisfy.
 *
 * Nothing here guesses. A quality or a language a mirror does not carry is absent rather than
 * derived from the name a second time; `rd_collector::mirrors` already read the release name
 * through a closed token list, and a second, looser reading in the browser would put `2160p`
 * on `Show.2160.mkv` after the server deliberately did not.
 */
import type { LinkCandidate } from '@/api/types'

import { hosterOf } from '@/utils/collectorSort'

/** The three dimensions a person may prefer, in the order the toolbar shows them. */
export const MIRROR_FACETS = ['quality', 'language', 'hoster'] as const
export type MirrorFacet = (typeof MIRROR_FACETS)[number]

/**
 * What is preferred, `null` per facet meaning "no preference", and which hosters are hidden.
 *
 * The hidden hosters (RD-130-21) are no fourth facet: a facet narrows the list to what matches,
 * they take named hosters out of it. They are stored with the facets because they are the same
 * kind of standing decision, and the server ranks a hidden hoster's mirror below a shown one.
 */
export type MirrorPreference = Record<MirrorFacet, string | null> & { hidden_hosters: string[] }

export const EMPTY_PREFERENCE: MirrorPreference = { quality: null, language: null, hoster: null, hidden_hosters: [] }

/** One mirror group as a single row: the chosen member, and the ones behind the chevron. */
export interface MirrorGroup {
  /** Key shared by the members; unique inside the package and meaningless outside it. */
  key: string
  source: 'declared' | 'name_and_size' | 'name'
  /** The member the package will download. */
  chosen: LinkCandidate
  /** Every member, chosen one included, in the package's order. */
  members: LinkCandidate[]
  /** Whether a person chose the current member by hand. */
  pinned: boolean
  /**
   * How many members a hoster confirmed as available; 0 means every mirror reads as gone.
   *
   * Not the same as "how many can be queued": every unhappy state except `unresolvable` means
   * *nothing good is known yet*, so a group of nothing but offline mirrors stays enqueueable
   * (RD-101-06). This number is what the row states, not what it disables.
   */
  onlineCount: number
}

/** A row of the LinkGrabber's link list: a lone link, or a whole mirror group. */
export type LinkRow =
  | { kind: 'link', candidate: LinkCandidate }
  | { kind: 'group', group: MirrorGroup }

/** The facet value a candidate carries, or `null` where nothing said. */
export function facetOf(candidate: LinkCandidate, facet: MirrorFacet): string | null {
  if (facet === 'hoster') return hosterOf(candidate) || null
  return candidate.mirror?.[facet] ?? null
}

/**
 * Whether a candidate satisfies every facet that is set.
 *
 * A facet the candidate says nothing about does **not** reject it. Hiding a link because
 * nobody could tell its quality would hide it for not being labelled, and the labelling is
 * exactly what this application refuses to invent. The hoster is the one facet every link
 * carries, so it is the one that really narrows a list of lone links.
 */
export function matchesPreference(candidate: LinkCandidate, preference: MirrorPreference): boolean {
  return MIRROR_FACETS.every((facet) => {
    const wanted = preference[facet]
    if (!wanted) return true
    const value = facetOf(candidate, facet)
    return value === null || value.toLowerCase() === wanted.toLowerCase()
  })
}

/** How many facets of the preference a candidate actually matches (not merely fails to deny). */
export function matchedFacets(candidate: LinkCandidate, preference: MirrorPreference): number {
  return MIRROR_FACETS.filter((facet) => {
    const wanted = preference[facet]
    if (!wanted) return false
    return facetOf(candidate, facet)?.toLowerCase() === wanted.toLowerCase()
  }).length
}

/**
 * Turns a package's candidates into its rows: one per lone link, one per mirror group.
 *
 * The group takes the position of its first member, so a package that is entirely one release
 * keeps the order it was submitted in and a reorder still lands where it looks like it will.
 */
export function mirrorRows(candidates: LinkCandidate[]): LinkRow[] {
  const rows: LinkRow[] = []
  const groups = new Map<string, MirrorGroup>()
  for (const candidate of candidates) {
    const mirror = candidate.mirror
    if (!mirror) {
      rows.push({ kind: 'link', candidate })
      continue
    }
    const existing = groups.get(mirror.group)
    if (existing) {
      existing.members.push(candidate)
      if (mirror.selected) {
        existing.chosen = candidate
        existing.pinned = Boolean(mirror.pinned)
      }
      if (candidate.state === 'online') existing.onlineCount += 1
      continue
    }
    const group: MirrorGroup = {
      key: mirror.group,
      source: mirror.source,
      // A group whose chosen member the server has not marked yet still needs a row; the
      // first member stands in, which is the same fallback the server itself starts from.
      chosen: candidate,
      members: [candidate],
      pinned: Boolean(mirror.selected && mirror.pinned),
      onlineCount: candidate.state === 'online' ? 1 : 0
    }
    groups.set(mirror.group, group)
    rows.push({ kind: 'group', group })
  }
  return rows
}

/**
 * Drops the rows the preference cannot satisfy.
 *
 * Where there is a choice the preference has already made it on the server, so a group is kept
 * whenever **any** of its members matches — the row then shows the mirror the server chose,
 * which is the mirror the queue will use. Where there is no choice the preference hides the
 * row. A group somebody pinned is never hidden: the decision was stated, and a default must
 * not take a stated decision off the screen.
 */
export function filterRows(rows: LinkRow[], preference: MirrorPreference): LinkRow[] {
  if (MIRROR_FACETS.every(facet => !preference[facet])) return rows
  return rows.filter((row) => {
    if (row.kind === 'link') return matchesPreference(row.candidate, preference)
    if (row.group.pinned) return true
    return row.group.members.some(member => matchesPreference(member, preference))
  })
}

/**
 * Drops the rows that only hidden hosters carry (RD-130-21).
 *
 * A lone link goes with its hoster. A group stays while **any** member sits at a shown hoster:
 * its hidden members are then the fallbacks the queue switches to, not links of their own, and
 * the server has already made a shown member the chosen one. `hidden` holds lowercased hosters.
 */
export function hideHosters(rows: LinkRow[], hidden: ReadonlySet<string>): LinkRow[] {
  if (!hidden.size) return rows
  const shown = (candidate: LinkCandidate) => !hidden.has(hosterOf(candidate).toLowerCase())
  return rows.filter(row => row.kind === 'link' ? shown(row.candidate) : row.group.members.some(shown))
}

/** The values a facet actually takes in this list, sorted, so the select offers no dead option. */
export function facetValues(candidates: LinkCandidate[], facet: MirrorFacet): string[] {
  const values = new Set<string>()
  for (const candidate of candidates) {
    const value = facetOf(candidate, facet)
    if (value) values.add(value)
  }
  return [...values].sort((a, b) => a.localeCompare(b))
}
