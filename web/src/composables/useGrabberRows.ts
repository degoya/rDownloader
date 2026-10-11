import { computed, type Ref } from 'vue'

import type { LinkCandidate } from '@/api/types'
import { grabberKey, isSelectableCandidate, mergeGrabberEntries, packageRowKey, type CollectorEntry, type NzbEntry } from '@/composables/useGrabberSelection'
import type { VirtualRow } from '@/composables/useVirtualRows'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { sortCandidates, sortCollectorEntries, type CollectorSort } from '@/utils/collectorSort'
import { filterRows, hideHosters, mirrorRows, type MirrorGroup } from '@/utils/mirrorGroups'

/**
 * The LinkGrabber as one stream of rows (RD-106-12).
 *
 * Package header, then its links while it is open, with reviewed NZB imports interleaved as
 * single rows. Collapsing filters this stream instead of hiding children inside a package,
 * because only a flat sequence can be windowed — and a row's key has to survive the window
 * moving past it, or the focus and the selection do not.
 */
interface GrabberPackageRow extends VirtualRow { kind: 'package', entry: CollectorEntry }
interface GrabberCandidateRow extends VirtualRow {
  kind: 'candidate'
  entry: CollectorEntry
  candidate: LinkCandidate
  /** Set when this row stands for a whole mirror group rather than for one link. */
  group?: MirrorGroup
  /** Set when this row is one of a group's other mirrors. */
  member?: boolean
}
interface GrabberNzbRow extends VirtualRow { kind: 'nzb', entry: NzbEntry }
type GrabberRow = GrabberPackageRow | GrabberCandidateRow | GrabberNzbRow

/** Starting estimates only; the list measures what the rows really are once they are drawn. */
const PACKAGE_ROW_SIZE = 48
const LINK_ROW_SIZE = 44
const NZB_ROW_SIZE = 48

interface OpenState { isOpen: (key: string) => boolean }

/**
 * What the LinkGrabber shows, derived from the stores and the view's sort and filters: the
 * packages with their visible links, the reviewed NZB imports, both in display order, and the
 * flattened rows the windowed list draws.
 */
export function useGrabberRows(view: {
  sort: Ref<CollectorSort>
  descending: Ref<boolean>
  stateFilter: Ref<LinkCandidate['state'] | 'all'>
  /** Hosters hidden from the list, lower case. */
  hidden: Readonly<Ref<ReadonlySet<string>>>
  /** Whether links a LinkFilter rule hid are drawn (RD-1240-09); hidden ones are left out otherwise. */
  showFiltered?: Readonly<Ref<boolean>>
  openPackages: OpenState
  openMirrors: OpenState
}) {
  const collector = useCollectorStore()
  const nzb = useNzbImportsStore()

  // Candidates are bucketed by package in one pass; filtering the whole list once per package
  // was O(packages x candidates) and re-ran on every refresh of a list that can hold thousands.
  const candidatesByPackage = computed(() => {
    const buckets = new Map<string, LinkCandidate[]>()
    for (const candidate of collector.candidates) {
      if (!candidate.package_id) continue
      if (view.stateFilter.value !== 'all' && candidate.state !== view.stateFilter.value) continue
      if (candidate.hidden_by_filter && !view.showFiltered?.value) continue
      const bucket = buckets.get(candidate.package_id)
      if (bucket) bucket.push(candidate)
      else buckets.set(candidate.package_id, [candidate])
    }
    // The facets act on whole mirror groups, so they are applied once the bucket is complete: a
    // per-candidate filter would take a 720p mirror out of a group that is being kept for its
    // 1080p one, and the group would then be missing a member nobody asked to hide.
    // Hidden hosters act on the same whole groups: a group stays while any member is at a shown
    // hoster, and its hidden members then go along as the fallbacks they are (RD-130-21).
    for (const [id, bucket] of buckets) {
      const kept = new Set(hideHosters(filterRows(mirrorRows(bucket), collector.mirrorPreference), view.hidden.value)
        .flatMap(row => row.kind === 'link' ? [row.candidate.id] : row.group.members.map(member => member.id)))
      if (kept.size !== bucket.length) buckets.set(id, bucket.filter(candidate => kept.has(candidate.id)))
    }
    return buckets
  })
  /** Manual sort keeps the stored package order so drag & drop stays visible. */
  const groups = computed<CollectorEntry[]>(() => sortCollectorEntries(collector.packages.map(pkg => ({
    kind: 'collector' as const,
    id: pkg.id,
    createdAt: pkg.created_at,
    position: pkg.position,
    package: pkg,
    candidates: sortCandidates(
      candidatesByPackage.value.get(pkg.id) ?? [],
      view.sort.value,
      view.descending.value
    )
  })).filter(entry => entry.candidates.length), view.sort.value, view.descending.value))
  const visibleLinks = computed(() => groups.value.reduce((count, group) => count + group.candidates.length, 0))
  const nzbGroups = computed<NzbEntry[]>(() => nzb.imports
    .filter(item => item.state === 'imported' || item.state === 'failed')
    .map(item => ({ kind: 'nzb' as const, id: item.id, createdAt: item.created_at, position: item.position, item }))
    .sort((a, b) => a.position - b.position || a.createdAt.localeCompare(b.createdAt)))
  // Non-manual sorts order the collector list by key, so the shared manual order would scramble it
  // again; NZB imports (which have no hoster or size) then keep their own block at the end.
  const entries = computed(() => view.sort.value === 'manual'
    ? mergeGrabberEntries(groups.value, nzbGroups.value)
    : [...groups.value, ...nzbGroups.value])

  const rows = computed<GrabberRow[]>(() => {
    const result: GrabberRow[] = []
    for (const entry of entries.value) {
      if (entry.kind === 'nzb') {
        result.push({ key: `nzb:${entry.id}`, size: NZB_ROW_SIZE, class: 'pt-2', kind: 'nzb', entry })
        continue
      }
      result.push({ key: `package:${entry.id}`, size: PACKAGE_ROW_SIZE, class: 'pt-2', kind: 'package', entry })
      if (!view.openPackages.isOpen(entry.id)) continue
      // One row per mirror group instead of one per link: a release page offering the same
      // episode at five hosters is one decision, not five (RD-110-19).
      for (const row of mirrorRows(entry.candidates)) {
        if (row.kind === 'link') {
          result.push({ key: `link:${row.candidate.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: row.candidate })
          continue
        }
        const group = row.group
        result.push({ key: `link:${group.chosen.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: group.chosen, group })
        if (!view.openMirrors.isOpen(group.key)) continue
        for (const member of group.members) {
          if (member.id === group.chosen.id) continue
          result.push({ key: `mirror:${member.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: member, member: true })
        }
      }
    }
    return result
  })
  /**
   * The order a range selection follows: what is on screen, not what is in the store. A package
   * row is a stop of its own, so a range can run from package to package (RD-170-13).
   */
  const orderedSelectionKeys = computed(() => rows.value.flatMap((row) => {
    if (row.kind === 'nzb') return [grabberKey('nzb', row.entry.id)]
    if (row.kind === 'package') return [packageRowKey(row.entry.id)]
    // A mirror of a group is not a candidate of its own: the group is what gets queued.
    if (row.kind === 'candidate' && !row.member && isSelectableCandidate(row.candidate)) return [grabberKey('collector', row.candidate.id)]
    return []
  }))

  return { groups, visibleLinks, nzbGroups, entries, rows, orderedSelectionKeys }
}
