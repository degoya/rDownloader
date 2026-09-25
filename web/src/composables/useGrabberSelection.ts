import { computed, ref, type Ref } from 'vue'

import type { CollectorPackage, LinkCandidate, NzbImport } from '@/api/types'

export type GrabberKind = 'collector' | 'nzb'

/** One collector package with the links currently displayed inside it. */
export interface CollectorEntry {
  kind: 'collector'
  id: string
  createdAt: string
  /** Place in the LinkGrabber's manual order, which both kinds now share. */
  position: number
  package: CollectorPackage
  candidates: LinkCandidate[]
}

/** One reviewed NZB import, rendered like a package. */
export interface NzbEntry {
  kind: 'nzb'
  id: string
  createdAt: string
  /** Place in the LinkGrabber's manual order, which both kinds now share. */
  position: number
  item: NzbImport
}

export type GrabberEntry = CollectorEntry | NzbEntry

/** Link states a user can act on; the others are transient or already queued. */
const SELECTABLE_STATES = ['online', 'duplicate', 'offline']

export function isSelectableCandidate(candidate: LinkCandidate): boolean {
  return SELECTABLE_STATES.includes(candidate.state)
}

export function grabberKey(kind: GrabberKind, id: string): string {
  return `${kind}:${id}`
}

/**
 * Puts both kinds into the one manual order the server keeps across them.
 *
 * This used to interleave by creation time, and that is what made an NZB row unmovable: it had
 * no place of its own in the order, only a timestamp that decided for it. Both listings carry a
 * `position` in a single sequence now, so the merge is a sort on it. `createdAt` and the id only
 * break a tie, which a listing that has not been renumbered yet can still produce — without them
 * the sort would not be stable across the two sources.
 */
export function mergeGrabberEntries(collector: CollectorEntry[], nzb: NzbEntry[]): GrabberEntry[] {
  return [...collector, ...nzb].sort((a, b) =>
    a.position - b.position || a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id))
}

/**
 * Selection across both entry kinds, keyed by `<kind>:<id>`.
 *
 * `orderedKeys` is the order the rows are on screen in — the flattened row stream of the
 * virtualized list, which leaves out whatever sits in a collapsed package. A range selection
 * follows that order rather than the store's (RD-106-12).
 */
export function useGrabberSelection(entries: Ref<GrabberEntry[]>, orderedKeys?: Ref<string[]>) {
  const selectedKeys = ref<Set<string>>(new Set())
  /** Where the last plain click landed; a shift-click selects from here to there. */
  const anchor = ref<string | null>(null)

  /** Every key the user is allowed to pick right now. */
  const selectableKeys = computed(() => entries.value.flatMap(entry => entry.kind === 'collector'
    ? entry.candidates.filter(isSelectableCandidate).map(candidate => grabberKey('collector', candidate.id))
    : [grabberKey('nzb', entry.id)]))

  function setKeys(keys: string[], selected: boolean): void {
    const next = new Set(selectedKeys.value)
    for (const key of keys) selected ? next.add(key) : next.delete(key)
    selectedKeys.value = next
  }

  function setCollector(ids: string[], selected: boolean): void {
    setKeys(ids.map(id => grabberKey('collector', id)), selected)
  }

  /**
   * Picks one row, or — holding shift — everything between the last plain pick and this one.
   *
   * The anchor stays put while the range is stretched, so a second shift-click corrects the
   * first instead of starting a new range.
   */
  function pick(key: string, selected: boolean, extend = false): void {
    const order = orderedKeys?.value ?? selectableKeys.value
    const from = anchor.value ? order.indexOf(anchor.value) : -1
    const to = order.indexOf(key)
    if (extend && from >= 0 && to >= 0) {
      const [low, high] = from <= to ? [from, to] : [to, from]
      setKeys(order.slice(low, high + 1), selected)
      return
    }
    anchor.value = key
    setKeys([key], selected)
  }

  function pickCollector(id: string, selected: boolean, extend = false): void {
    pick(grabberKey('collector', id), selected, extend)
  }

  function pickNzb(id: string, selected: boolean, extend = false): void {
    pick(grabberKey('nzb', id), selected, extend)
  }

  /** Stale keys (deleted or re-checked entries) drop out here instead of being persisted. */
  const activeKeys = computed(() => selectableKeys.value.filter(key => selectedKeys.value.has(key)))
  const collectorIds = computed(() => activeKeys.value.filter(key => key.startsWith('collector:')).map(key => key.slice('collector:'.length)))
  const nzbIds = computed(() => activeKeys.value.filter(key => key.startsWith('nzb:')).map(key => key.slice('nzb:'.length)))
  /** Raw candidate ids for `CollectorPackageGroup`, which selects by link id. */
  const collectorIdSet = computed(() => new Set(collectorIds.value))
  const count = computed(() => activeKeys.value.length)
  const state = computed<'none' | 'some' | 'all'>(() => {
    if (!count.value) return 'none'
    return count.value === selectableKeys.value.length ? 'all' : 'some'
  })

  function selectAll(): void {
    selectedKeys.value = new Set(selectableKeys.value)
  }

  function clear(): void {
    selectedKeys.value = new Set()
    anchor.value = null
  }

  /** Header control: anything selected clears, nothing selected picks everything. */
  function toggleAll(): void {
    state.value === 'none' ? selectAll() : clear()
  }

  function isNzbSelected(id: string): boolean {
    return selectedKeys.value.has(grabberKey('nzb', id))
  }

  return { selectedKeys, collectorIds, collectorIdSet, nzbIds, count, state, anchor, setCollector, pick, pickCollector, pickNzb, selectAll, clear, toggleAll, isNzbSelected }
}
