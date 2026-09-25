import type { LinkCandidate } from '@/api/types'
import { i18n } from '@/i18n'

export type CollectorSort = 'manual' | 'name' | 'hoster' | 'size' | 'added'

/** Sort options with their label keys; resolve the labels with `t` inside a `computed`. */
export const SORT_OPTIONS: { labelKey: string, value: CollectorSort }[] = [
  { labelKey: 'linkgrabber.sort.manual', value: 'manual' },
  { labelKey: 'linkgrabber.sort.name', value: 'name' },
  { labelKey: 'linkgrabber.sort.hoster', value: 'hoster' },
  { labelKey: 'linkgrabber.sort.size', value: 'size' },
  { labelKey: 'linkgrabber.sort.added', value: 'added' }
]

function compareNames(a: LinkCandidate, b: LinkCandidate, sensitivity?: 'base'): number {
  return displayName(a).localeCompare(displayName(b), i18n.global.locale.value, { numeric: true, ...(sensitivity ? { sensitivity } : {}) })
}

export function hosterOf(candidate: LinkCandidate): string {
  try {
    return new URL(candidate.url).hostname.replace(/^www\./, '') || candidate.provider || ''
  } catch {
    return candidate.provider ?? ''
  }
}

/**
 * The label a row shows and sorts by. A rename writes `file_name`, so it always wins; media
 * candidates fall back to the page title because their URL tail (`watch?v=…`) is meaningless.
 */
export function displayName(candidate: LinkCandidate): string {
  return candidate.file_name || candidate.media?.title || candidate.url.split('/').pop() || candidate.url
}

/** Sorts candidates inside a package; manual keeps the stored position order. */
export function sortCandidates(candidates: LinkCandidate[], sort: CollectorSort, descending: boolean): LinkCandidate[] {
  const sorted = [...candidates]
  const direction = descending ? -1 : 1
  switch (sort) {
    case 'name':
      sorted.sort((a, b) => direction * compareNames(a, b, 'base'))
      break
    case 'hoster':
      sorted.sort((a, b) => direction * (hosterOf(a).localeCompare(hosterOf(b), i18n.global.locale.value) || compareNames(a, b)))
      break
    case 'size':
      sorted.sort((a, b) => {
        const left = a.size ? BigInt(a.size) : -1n
        const right = b.size ? BigInt(b.size) : -1n
        return direction * (left < right ? -1 : left > right ? 1 : 0)
      })
      break
    case 'added':
      sorted.sort((a, b) => direction * (a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id)))
      break
    default:
      sorted.sort((a, b) => (a.position ?? 0) - (b.position ?? 0))
  }
  return sorted
}

/** Structural subset of the grabber's collector entry (the full type lives in useGrabberSelection). */
interface SortableCollectorEntry {
  createdAt: string
  package: { name: string }
  candidates: LinkCandidate[]
}

function firstHoster(entry: SortableCollectorEntry): string {
  const first = entry.candidates[0]
  return first ? hosterOf(first) : ''
}

function totalSize(entry: SortableCollectorEntry): bigint {
  return entry.candidates.reduce((sum, candidate) => sum + (candidate.size ? BigInt(candidate.size) : 0n), 0n)
}

/**
 * Sorts whole packages by the same key as their candidates, so a sort visibly reorders the
 * list instead of only shuffling links inside each package. Candidates must already be sorted
 * (the hoster key uses the first candidate as the package's representative).
 */
export function sortCollectorEntries<T extends SortableCollectorEntry>(entries: T[], sort: CollectorSort, descending: boolean): T[] {
  if (sort === 'manual') return entries
  const direction = descending ? -1 : 1
  const sorted = [...entries]
  switch (sort) {
    case 'name':
      sorted.sort((a, b) => direction * a.package.name.localeCompare(b.package.name, i18n.global.locale.value, { numeric: true, sensitivity: 'base' }))
      break
    case 'hoster':
      sorted.sort((a, b) => direction * firstHoster(a).localeCompare(firstHoster(b), i18n.global.locale.value))
      break
    case 'size':
      sorted.sort((a, b) => {
        const left = totalSize(a)
        const right = totalSize(b)
        return direction * (left < right ? -1 : left > right ? 1 : 0)
      })
      break
    case 'added':
      sorted.sort((a, b) => direction * a.createdAt.localeCompare(b.createdAt))
      break
  }
  return sorted
}
