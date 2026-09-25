import { useToast } from '@nuxt/ui/composables'
import type { Ref } from 'vue'
import type { LinkCandidate } from '@/api/types'
import { useI18n } from 'vue-i18n'

import { useConfirm } from '@/composables/useConfirm'
import { isSelectableCandidate, type CollectorEntry, type NzbEntry } from '@/composables/useGrabberSelection'
import { useReplayConsent } from '@/composables/useReplayConsent'
import { useCollectorStore, type EnqueueBatchResult } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { useTransfersStore } from '@/stores/transfers'
import type { CollectorSort } from '@/utils/collectorSort'

/**
 * Handing links and NZB imports over to the downloader (RD-106-12).
 *
 * The eight functions this holds were the other half of what made `LinkGrabberView` twice the
 * length the rest of this codebase keeps to. They belong together: every route into the queue —
 * the whole list, one package, a selection, a single link — goes through the same three steps of
 * storing the displayed order, asking for consent where a captured request would send
 * credentials, and reporting one combined result rather than one toast per link.
 *
 * It takes what the view displays rather than reading it, because the order that gets stored is
 * the order on screen: the arrangement the reader made, not the one the server last saw.
 */
export function useGrabberEnqueue(view: {
  /** The packages as displayed, with their candidates in displayed order. */
  groups: Ref<CollectorEntry[]>
  /** The NZB imports as displayed. */
  nzbGroups: Ref<NzbEntry[]>
  /** The imports that may still be queued — a duplicate may not. */
  enqueueableNzbIds: Ref<string[]>
  /** The current selection across both lists. */
  selection: {
    collectorIds: Ref<string[]>
    nzbIds: Ref<string[]>
    clear: () => void
  }
  /** The active sort; under `manual` the displayed order is already the stored one. */
  sort: Ref<CollectorSort>
  /** True while a facet, the state filter or a hidden hoster (RD-130-21) hides part of the list. */
  filterActive: Ref<boolean>
  /** Where a refusal to store a partial order is shown. */
  notice: Ref<string | null>
  /** Raised while a bulk enqueue is in flight, for the action bar that started it. */
  bulkBusy: Ref<boolean>
}) {
  const { t } = useI18n()
  const toast = useToast()
  const confirm = useConfirm()
  const replayConsent = useReplayConsent()
  const collector = useCollectorStore()
  const nzb = useNzbImportsStore()
  const transfers = useTransfersStore()

  async function persistDisplayedOrder(packageIds: string[]): Promise<void> {
    // A manual sort already shows the stored order, so there is nothing to write.
    if (view.sort.value === 'manual') return
    // With a filter active the displayed lists are partial; persisting them would drop hidden links
    // from the stored order, so the enqueue falls back to the stored positions instead — and says
    // so, because the sequence the user arranged on screen is not the one being enqueued.
    if (view.filterActive.value) {
      view.notice.value = t('linkgrabber.notices.reorder_filter_active')
      return
    }
    view.notice.value = null
    // One round trip per package, but in parallel: awaiting them in sequence made enqueuing ten
    // packages take ten times as long as it needed to.
    await Promise.all(view.groups.value
      .filter(g => packageIds.includes(g.package.id))
      .map(group => collector.reorderCandidates(group.package.id, group.candidates.map(c => c.id))))
  }

  /**
   * The links of these packages on screen, or `undefined` while no filter hides any.
   *
   * A filter hides links inside a package, and the server enqueues a package whole unless it is
   * told which links to take — so a queue filtered to one hoster used to receive every other
   * hoster's links as well. Sent, the hidden ones stay in the LinkGrabber, in their package.
   *
   * A shown link's mirrors go along even when the filter hides them: they are not downloads of
   * their own but the fallbacks the queue switches to when the chosen mirror fails (RD-110-20),
   * and a quality or language preference hides every mirror but the chosen one. 1.2.4 left them
   * behind, and a failed mirror then had nothing to fall back to.
   */
  function visibleCandidateIds(packageIds: string[]): string[] | undefined {
    if (!view.filterActive.value) return undefined
    return withMirrors(view.groups.value.filter(g => packageIds.includes(g.package.id)).flatMap(g => g.candidates))
  }

  /** These links' ids, followed by the ids of every other member of their mirror groups. */
  function withMirrors(links: LinkCandidate[]): string[] {
    const ids = new Set(links.map(c => c.id))
    // A group key is unique within its package only, so package and key together name a group.
    const groups = new Set(links.filter(c => c.mirror).map(c => `${c.package_id}\u0000${c.mirror?.group}`))
    const mirrors = collector.candidates.filter(c => c.mirror && !ids.has(c.id)
      && groups.has(`${c.package_id}\u0000${c.mirror.group}`))
    return [...ids, ...mirrors.map(c => c.id)]
  }

  async function enqueueAll(paused = false): Promise<void> {
    const ids = view.groups.value.filter(g => g.candidates.some(c => c.state === 'online' || c.state === 'duplicate')).map(g => g.package.id)
    const nzbIds = view.enqueueableNzbIds.value
    if (!ids.length && !nzbIds.length) return
    if (view.groups.value.some(g => g.candidates.some(c => c.state === 'duplicate'))) {
      const confirmed = await confirm({
        title: t('linkgrabber.confirm.duplicates_title'),
        description: paused ? t('linkgrabber.confirm.duplicates_paused') : t('linkgrabber.confirm.duplicates_enqueue'),
        confirmLabel: paused ? t('linkgrabber.actions.enqueue_paused') : t('linkgrabber.actions.enqueue_all'),
        confirmIcon: paused ? 'i-lucide-pause' : 'i-lucide-list-end'
      })
      if (!confirmed) return
    }
    await persistDisplayedOrder(ids)
    const [links, imports] = await Promise.all([collector.enqueuePackages(ids, paused, visibleCandidateIds(ids)), nzb.enqueueMany(nzbIds, paused)])
    await finishEnqueue(links, imports)
  }

  /** `paused` is the start-mode variant the row offers beside this one; see `design.md`. */
  async function enqueuePackage(id: string, paused = false): Promise<void> {
    const visible = visibleCandidateIds([id])
    const members = visible ?? collector.candidates.filter(c => c.package_id === id).map(c => c.id)
    if (!(await ensureReplayConsent(members))) return
    await persistDisplayedOrder([id])
    const result = await collector.enqueuePackages([id], paused, visible)
    reportEnqueueExtras(result)
    if (result.created) await transfers.refresh()
  }

  const EMPTY_ENQUEUE: EnqueueBatchResult = { created: 0, failed: 0, firstError: null, freeDownloadFiles: 0 }

  /** Enqueues the selected links; partially selected packages are split off first. */
  async function enqueueSelectedCollector(paused: boolean): Promise<EnqueueBatchResult> {
    const selected = view.selection.collectorIds.value
    if (!selected.length) return EMPTY_ENQUEUE
    // Selecting a mirror group selects its chosen link; its fallbacks count as selected with it,
    // for the split and for which packages go to the queue.
    const shown = view.groups.value.flatMap(g => g.candidates)
    const ids = withMirrors(shown.filter(c => selected.includes(c.id)))
    const packageIds = [...new Set(view.groups.value.filter(g => g.candidates.some(c => ids.includes(c.id))).map(g => g.package.id))]
    // Splitting partially selected packages is independent per package, so these go out together
    // rather than one awaited request after another.
    await Promise.all(packageIds.map((packageId) => {
      const group = view.groups.value.find(g => g.package.id === packageId)
      if (!group) return undefined
      const chosen = ids.filter(id => collector.candidates.some(c => c.id === id && c.package_id === packageId))
      // Compare against the unfiltered package size: an active filter hides candidates that must
      // not be enqueued along with a "fully" selected filtered view.
      const packageTotal = collector.candidates.filter(c => c.package_id === packageId).length
      if (chosen.length === packageTotal) return undefined
      return collector.moveCandidates(chosen, { newPackageName: t('linkgrabber.selection_package', { name: group.package.name }) })
    }))
    const targets = view.groups.value.filter(g => g.candidates.every(c => ids.includes(c.id) || !isSelectableCandidate(c)) && g.candidates.some(c => ids.includes(c.id))).map(g => g.package.id)
    await persistDisplayedOrder(targets)
    return collector.enqueuePackages(targets, paused)
  }

  async function enqueueSelected(paused = false): Promise<void> {
    view.bulkBusy.value = true
    const nzbIds = view.selection.nzbIds.value.filter(id => view.nzbGroups.value.some(entry => entry.id === id && !entry.item.duplicate))
    const [links, imports] = await Promise.all([enqueueSelectedCollector(paused), nzb.enqueueMany(nzbIds, paused)])
    view.bulkBusy.value = false
    await finishEnqueue(links, imports)
    // Cleared last: dropping it first made the action bar vanish while the rows it acted on were
    // still listed, which is what made the enqueue look stuck.
    view.selection.clear()
  }

  /** Surfaces partial failures and account-less (free/direct) files of a batch enqueue. */
  function reportEnqueueExtras(result: EnqueueBatchResult): void {
    if (result.freeDownloadFiles) {
      toast.add({ title: t('linkgrabber.bulk.free_download', { count: result.freeDownloadFiles }, result.freeDownloadFiles), color: 'info', icon: 'i-lucide-user-x' })
    }
    if (result.failed) {
      toast.add({ title: t('linkgrabber.bulk.enqueue_failed', { count: result.failed }, result.failed), ...(result.firstError ? { description: result.firstError } : {}), color: 'warning', icon: 'i-lucide-circle-alert' })
    }
  }

  /** Refreshes both lists once and reports the combined result of a mixed enqueue. */
  async function finishEnqueue(links: EnqueueBatchResult, imports: number): Promise<void> {
    reportEnqueueExtras(links)
    if (!links.created && !imports) return
    await Promise.all([collector.refresh(), transfers.refresh()])
    toast.add({ title: t('linkgrabber.bulk.enqueued', { links: links.created, nzbs: imports }), color: 'success', icon: 'i-lucide-list-end' })
  }

  /**
   * Asks for approval when a captured request would send credentials.
   *
   * The server enforces this regardless; the dialog is how a person sees what they are
   * approving. Returns false when the user declined, so the caller stops.
   */
  async function ensureReplayConsent(ids: string[]): Promise<boolean> {
    for (const id of ids) {
      const candidate = collector.candidates.find(c => c.id === id)
      const request = candidate?.request
      if (!request) continue
      const sendsCredentials = request.method !== 'GET' || Boolean(request.body) || Boolean(request.expires_at)
      if (!sendsCredentials || candidate?.replay_consent) continue
      const preview = await collector.replayPreview(id)
      if (!preview) return false
      const result = await replayConsent(preview, !preview.replayable)
      if (!result) return false
      if (!(await collector.grantReplayConsent(id, preview.template_hash, result.approvedOrigins))) return false
    }
    return true
  }

  async function enqueueCandidate(id: string): Promise<void> {
    const candidate = collector.candidates.find(c => c.id === id)
    if (!(await ensureReplayConsent([id]))) return
    if (candidate?.state === 'duplicate') {
      const confirmed = await confirm({ title: t('linkgrabber.confirm.duplicate_title'), description: t('linkgrabber.confirm.duplicate_description'), confirmLabel: t('linkgrabber.confirm.duplicate_confirm'), confirmIcon: 'i-lucide-copy-plus' })
      if (!confirmed) return
    }
    if (await collector.enqueueCandidate(id)) await transfers.refresh()
  }

  return { persistDisplayedOrder, enqueueAll, enqueuePackage, enqueueSelected, finishEnqueue, enqueueCandidate, visibleCandidateIds }
}
