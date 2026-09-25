import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useCollectorStore, type GrabberOrderEntry } from '@/stores/collector'
import { grabberKey, mergeGrabberEntries, type CollectorEntry, type GrabberKind, type NzbEntry } from '@/composables/useGrabberSelection'
import type { CollectorSort } from '@/utils/collectorSort'

/** The part of the list component this needs: putting focus back on a row that has moved. */
export interface RowFocus {
  focusRow: (key: string) => Promise<boolean>
}

/**
 * Arranging packages and the links inside them, by drag or by keyboard (RD-106-12).
 *
 * Four functions in `LinkGrabberView` did this, and three of them opened with the same guard
 * written out again: reordering while a filter hides part of a package would persist an order
 * that is missing the hidden links, so the move is refused and the reader is told why. Stating
 * that once is the reason this is a composable — the fourth copy is the one that forgets, and
 * what it costs is a stored order with links dropped out of it.
 *
 * Every move also forces the sort back to manual. An arrangement the reader made by hand is not
 * visible under a key sort, so leaving the sort alone would discard the move on screen while
 * saving it on the server. Which is also why the order that gets stored is built from the
 * entries' `position`, never from the sequence a key sort happens to be showing: the move is
 * applied to the manual order, and switching back to manual is what makes it visible.
 *
 * Packages and NZB imports are *one* order here. They were two — a package had a drag handle
 * and a manual position, an import had neither and sat wherever its timestamp put it — so the
 * same list held two kinds of row that behaved differently for no reason a reader could see.
 */
export function useGrabberReorder(view: {
  /** The packages as displayed, with their candidates in displayed order. */
  groups: Ref<CollectorEntry[]>
  /** The reviewed NZB imports as displayed; they share the packages' order. */
  nzbGroups: Ref<NzbEntry[]>
  /** The active sort; a manual arrangement sets it back to `manual`. */
  sort: Ref<CollectorSort>
  /** True while a hoster or state filter hides part of the list. */
  filterActive: Ref<boolean>
  /** Where the refusal above is shown. */
  notice: Ref<string | null>
  /** The windowed list, so a moved row keeps the focus that moved it. */
  list: Ref<RowFocus | null>
}) {
  const { t } = useI18n()
  const collector = useCollectorStore()
  /** The entry being dragged, as `<kind>:<id>` — one ref for both kinds of row. */
  const draggingEntry = ref<string | null>(null)
  const draggingCandidate = ref<string | null>(null)

  /** False when a partial view makes the arrangement unsafe to store; says so and stops. */
  function mayReorder(): boolean {
    if (view.filterActive.value) {
      view.notice.value = t('linkgrabber.notices.reorder_filter_active')
      return false
    }
    view.notice.value = null
    return true
  }

  /** Moves `id` to where `targetId` sits, leaving the rest of `order` in place. */
  function moved(order: string[], id: string, targetId: string): string[] {
    const rest = order.filter(entry => entry !== id)
    rest.splice(rest.indexOf(targetId), 0, id)
    return rest
  }

  /**
   * The manual order of the whole list, as `<kind>:<id>` keys.
   *
   * Taken from the entries the view holds — the right *set*, including the imports — but put
   * back into `position` order, so a drag under a key sort still moves the row within the
   * arrangement it will be shown in once the sort flips back to manual.
   */
  function orderKeys(): string[] {
    return mergeGrabberEntries(view.groups.value, view.nzbGroups.value)
      .map(entry => grabberKey(entry.kind, entry.id))
  }

  /**
   * Stores the move as the one row that moved, anchored behind the row it now follows.
   *
   * The endpoint lifts the listed entries out of the shared order and splices them back in
   * directly behind the anchor, leaving every other row where it was. So a move is one entry and
   * one anchor whether it happened at the top of the list or three thousand rows down. Sending
   * the stretch between the old place and the new one instead makes the request grow with the
   * distance dragged, which is how a long drag ran into the endpoint's size bound — the bound is
   * there to limit how many rows one request *moves*, not how far it moves them.
   *
   * No anchor means the head of the list: the row moved to the front and has nothing in front.
   */
  async function persistOrder(key: string, before: string[], after: string[]): Promise<void> {
    if (after.every((entry, index) => entry === before[index])) return
    const index = after.indexOf(key)
    if (index < 0) return
    const anchor = index > 0 ? after[index - 1] : undefined
    await collector.reorderEntries([orderEntryOf(key)], anchor ? orderEntryOf(anchor) : null)
  }

  /** The shared tail of both row drops: move the dragged entry to where the target sits. */
  async function dropEntry(targetKey: string): Promise<void> {
    const sourceKey = draggingEntry.value
    draggingEntry.value = null
    if (!sourceKey || sourceKey === targetKey) return
    if (!mayReorder()) return
    view.sort.value = 'manual'
    const before = orderKeys()
    await persistOrder(sourceKey, before, moved(before, sourceKey, targetKey))
  }

  async function dropOnPackage(targetId: string): Promise<void> {
    if (draggingCandidate.value) {
      const id = draggingCandidate.value
      draggingCandidate.value = null
      const candidate = collector.candidates.find(c => c.id === id)
      if (candidate && candidate.package_id !== targetId) await collector.moveCandidates([id], { packageId: targetId })
      return
    }
    await dropEntry(grabberKey('collector', targetId))
  }

  async function dropOnNzb(targetId: string): Promise<void> {
    // A link dropped on an import has nowhere to land: an NZB import is not a collector package
    // and holds no candidates. The drag is dropped rather than turned into something else.
    if (draggingCandidate.value) {
      draggingCandidate.value = null
      return
    }
    await dropEntry(grabberKey('nzb', targetId))
  }

  async function dropOnCandidate(targetId: string): Promise<void> {
    const sourceId = draggingCandidate.value
    draggingCandidate.value = null
    if (!sourceId || sourceId === targetId) return
    const source = collector.candidates.find(c => c.id === sourceId)
    const target = collector.candidates.find(c => c.id === targetId)
    if (!source || !target || !target.package_id) return
    // Across packages this is a move, not an arrangement, so the filter guard does not apply.
    if (source.package_id !== target.package_id) {
      await collector.moveCandidates([sourceId], { packageId: target.package_id })
      return
    }
    if (!mayReorder()) return
    view.sort.value = 'manual'
    const group = view.groups.value.find(g => g.package.id === target.package_id)
    if (!group) return
    const order = moved(group.candidates.map(c => c.id), sourceId, targetId)
    if (await collector.reorderCandidates(target.package_id, order)) await collector.refresh()
  }

  /**
   * Keyboard counterpart of the candidate drag: moves one link a single step inside its package.
   *
   * Reordering has to be reachable without a pointer, and a drag never is.
   */
  async function moveCandidate(id: string, delta: -1 | 1): Promise<void> {
    const candidate = collector.candidates.find(c => c.id === id)
    if (!candidate?.package_id) return
    if (!mayReorder()) return
    const group = view.groups.value.find(g => g.package.id === candidate.package_id)
    if (!group) return
    const order = group.candidates.map(c => c.id)
    if (!step(order, id, delta)) return
    view.sort.value = 'manual'
    if (await collector.reorderCandidates(candidate.package_id, order)) await collector.refresh()
    // The row has moved, and with a windowed list its new place may be outside what is rendered.
    // Focus is put back on the same handle so a second press continues the move (RD-106-12).
    await view.list.value?.focusRow(`link:${id}`)
  }

  /**
   * Keyboard counterpart of the row drag: one step up or down in the merged order.
   *
   * Both kinds go through it, because both kinds have a handle now and a handle that a drag
   * can use but the arrow keys cannot is a control half the audience cannot reach.
   */
  async function moveEntry(kind: GrabberKind, id: string, delta: -1 | 1): Promise<void> {
    if (!mayReorder()) return
    const before = orderKeys()
    const after = [...before]
    if (!step(after, grabberKey(kind, id), delta)) return
    view.sort.value = 'manual'
    await persistOrder(grabberKey(kind, id), before, after)
    // The row has moved, and with a windowed list its new place may be outside what is rendered.
    // Focus is put back on the same handle so a second press continues the move (RD-106-12).
    await view.list.value?.focusRow(kind === 'nzb' ? `nzb:${id}` : `package:${id}`)
  }

  return { draggingEntry, draggingCandidate, dropOnPackage, dropOnNzb, dropOnCandidate, moveCandidate, moveEntry }
}

/** Splits a `<kind>:<id>` row key back into the pair the endpoint names an entry by. */
function orderEntryOf(key: string): GrabberOrderEntry {
  const separator = key.indexOf(':')
  return { kind: key.slice(0, separator) as GrabberKind, id: key.slice(separator + 1) }
}

/** Moves `id` one place within `order`, in place. False at either end, where there is nowhere to go. */
function step(order: string[], id: string, delta: -1 | 1): boolean {
  const from = order.indexOf(id)
  const to = from + delta
  if (from < 0 || to < 0 || to >= order.length) return false
  order.splice(to, 0, ...order.splice(from, 1))
  return true
}
