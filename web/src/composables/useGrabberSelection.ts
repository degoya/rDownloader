import { computed, ref, type Ref } from 'vue'

import type { CollectorPackage, LinkCandidate, NzbImport } from '@/api/types'
import { useRangeSelection } from '@/composables/useRangeSelection'
import { isEnqueueable } from '@/utils/candidateState'
import { sumSelection } from '@/utils/selectionSize'

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

/**
 * Link states a user can act on: the ones a link may be queued from, so a row that shows a
 * checkbox can also be ticked. The others are transient or already queued.
 */
export function isSelectableCandidate(candidate: LinkCandidate): boolean {
  return isEnqueueable(candidate.state)
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

/** Key of a package row in the range order; it stands for the package's selectable links. */
export function packageRowKey(id: string): string {
  return `package:${id}`
}

/**
 * Selection across both entry kinds, keyed by `<kind>:<id>`.
 *
 * `orderedKeys` is the order the rows are on screen in — the flattened row stream of the
 * virtualized list, which leaves out whatever sits in a collapsed package. A range selection
 * follows that order rather than the store's (RD-106-12); a package row takes part in it as
 * `package:<id>` and brings its links along (RD-170-13).
 */
export function useGrabberSelection(entries: Ref<GrabberEntry[]>, orderedKeys?: Ref<string[]>) {
  const selectedKeys = ref<Set<string>>(new Set())

  const linkKeys = (entry: CollectorEntry) => entry.candidates.filter(isSelectableCandidate).map(candidate => grabberKey('collector', candidate.id))

  /** Every key the user is allowed to pick right now. */
  const selectableKeys = computed(() => entries.value.flatMap(entry => entry.kind === 'collector' ? linkKeys(entry) : [grabberKey('nzb', entry.id)]))

  /** The selectable links behind each package row. */
  const packageKeys = computed(() => new Map(entries.value.flatMap(entry => entry.kind === 'collector' ? [[packageRowKey(entry.id), linkKeys(entry)] as const] : [])))

  /** Without the view's row stream: every package row followed by its links, in entry order. */
  const defaultOrder = computed(() => entries.value.flatMap(entry => entry.kind === 'collector'
    ? [packageRowKey(entry.id), ...linkKeys(entry)]
    : [grabberKey('nzb', entry.id)]))

  function setKeys(keys: string[], selected: boolean): void {
    const next = new Set(selectedKeys.value)
    for (const key of keys) selected ? next.add(key) : next.delete(key)
    selectedKeys.value = next
  }

  const range = useRangeSelection(
    computed(() => orderedKeys?.value ?? defaultOrder.value),
    setKeys,
    key => packageKeys.value.get(key)
  )
  const anchor = range.anchor

  function setCollector(ids: string[], selected: boolean): void {
    setKeys(ids.map(id => grabberKey('collector', id)), selected)
  }

  /**
   * Picks one row, or — holding shift — everything between the last plain pick and this one.
   * `extend` defaults to the modifier the list noted for this click (`noteModifier`).
   */
  function pick(key: string, selected: boolean, extend?: boolean): void {
    range.pick(key, selected, extend)
  }

  function pickCollector(id: string, selected: boolean, extend?: boolean): void {
    pick(grabberKey('collector', id), selected, extend)
  }

  function pickNzb(id: string, selected: boolean, extend?: boolean): void {
    pick(grabberKey('nzb', id), selected, extend)
  }

  /** The package checkbox: all of its selectable links, or a range of rows ending here. */
  function pickPackage(id: string, selected: boolean, extend?: boolean): void {
    pick(packageRowKey(id), selected, extend)
  }

  /** Stale keys (deleted or re-checked entries) drop out here instead of being persisted. */
  const activeKeys = computed(() => selectableKeys.value.filter(key => selectedKeys.value.has(key)))
  const collectorIds = computed(() => activeKeys.value.filter(key => key.startsWith('collector:')).map(key => key.slice('collector:'.length)))
  const nzbIds = computed(() => activeKeys.value.filter(key => key.startsWith('nzb:')).map(key => key.slice('nzb:'.length)))
  /** Raw candidate ids for `CollectorPackageGroup`, which selects by link id. */
  const collectorIdSet = computed(() => new Set(collectorIds.value))
  const count = computed(() => activeKeys.value.length)
  /**
   * The status bar's figure: one size per selected link or import (RD-170-14). A package row is
   * never a key of its own, so a ticked package and its ticked links count once.
   */
  const size = computed(() => {
    const sizes = new Map<string, string | null | undefined>()
    for (const entry of entries.value) {
      if (entry.kind === 'nzb') sizes.set(grabberKey('nzb', entry.id), entry.item.total_bytes)
      else for (const candidate of entry.candidates) sizes.set(grabberKey('collector', candidate.id), candidate.size)
    }
    return sumSelection(activeKeys.value.map(key => sizes.get(key)))
  })
  const state = computed<'none' | 'some' | 'all'>(() => {
    if (!count.value) return 'none'
    return count.value === selectableKeys.value.length ? 'all' : 'some'
  })

  function selectAll(): void {
    selectedKeys.value = new Set(selectableKeys.value)
  }

  function clear(): void {
    selectedKeys.value = new Set()
    range.reset()
  }

  /** Header control: anything selected clears, nothing selected picks everything. */
  function toggleAll(): void {
    state.value === 'none' ? selectAll() : clear()
  }

  function isNzbSelected(id: string): boolean {
    return selectedKeys.value.has(grabberKey('nzb', id))
  }

  return { selectedKeys, collectorIds, collectorIdSet, nzbIds, count, size, state, anchor, noteModifier: range.noteModifier, setCollector, pick, pickCollector, pickNzb, pickPackage, selectAll, clear, toggleAll, isNzbSelected }
}
