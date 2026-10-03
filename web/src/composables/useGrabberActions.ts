import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DownloadPriority, PostprocessLevel } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import type { NzbEntry } from '@/composables/useGrabberSelection'
import { packageEditChange, usePackageEdit } from '@/composables/usePackageEdit'
import { useRename } from '@/composables/useRename'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { useTransfersStore } from '@/stores/transfers'

/**
 * What a person does to rows of the LinkGrabber other than queue or reorder them: remove,
 * rename, edit, move into a new package, and set category, priority or post-processing — on
 * one row or over the selection. Each destructive one asks first.
 */
export function useGrabberActions(view: {
  /** The reviewed NZB imports as displayed; "delete all" clears them with the links. */
  nzbGroups: Ref<NzbEntry[]>
  /** The current selection across both lists. */
  selection: {
    count: Ref<number>
    collectorIds: Ref<string[]>
    nzbIds: Ref<string[]>
    clear: () => void
  }
  /** Set while a bulk action runs, so the bar shows it. */
  bulkBusy: Ref<boolean>
}) {
  const { t } = useI18n()
  const collector = useCollectorStore()
  const nzb = useNzbImportsStore()
  const transfers = useTransfersStore()
  const confirm = useConfirm()
  const editPackage = usePackageEdit()
  const rename = useRename()
  const { selection, bulkBusy } = view
  /** How far a removal of the selection has got, while it runs. */
  const removeProgress = ref<{ done: number, total: number } | null>(null)

  async function removeSelected(): Promise<void> {
    const count = selection.count.value
    const confirmed = await confirm({ title: t('linkgrabber.confirm.remove_selected_title'), description: t('linkgrabber.confirm.remove_selected_description', { count }, count), confirmLabel: t('common.actions.delete'), confirmIcon: 'i-lucide-trash-2', destructive: true })
    if (!confirmed) return
    bulkBusy.value = true
    const nzbIds = [...selection.nzbIds.value]
    const linkIds = [...selection.collectorIds.value]
    const total = linkIds.length + nzbIds.length
    removeProgress.value = { done: 0, total }
    await collector.deleteCandidates(linkIds, (done) => { removeProgress.value = { done, total } })
    for (const [index, id] of nzbIds.entries()) {
      await nzb.remove(id)
      removeProgress.value = { done: linkIds.length + index + 1, total }
    }
    removeProgress.value = null
    bulkBusy.value = false
    selection.clear()
  }

  async function moveSelected(): Promise<void> {
    const name = await rename({ title: t('linkgrabber.confirm.move_title'), label: t('linkgrabber.confirm.package_name'), value: '', maxLength: 200 })
    if (!name) return
    await collector.moveCandidates(selection.collectorIds.value, { newPackageName: name })
    selection.clear()
  }

  async function editPackageDialog(id: string): Promise<void> {
    const pkg = collector.packages.find(item => item.id === id)
    if (!pkg) return
    const result = await editPackage({ name: pkg.name, hasPassword: pkg.has_password, password: pkg.password ?? null, postprocessLevel: pkg.postprocess_level ?? null, script: pkg.script ?? null })
    if (!result) return
    const change = packageEditChange(pkg, result)
    if (Object.keys(change).length) await collector.updatePackages([id], change)
  }

  async function renameCandidate(id: string): Promise<void> {
    const candidate = collector.candidates.find(c => c.id === id)
    if (!candidate) return
    const name = await rename({ title: t('linkgrabber.confirm.rename_title'), label: t('linkgrabber.confirm.file_name'), value: candidate.file_name ?? '', description: candidate.url })
    if (name) await collector.renameCandidate(id, name)
  }

  async function removePackage(id: string): Promise<void> {
    const pkg = collector.packages.find(item => item.id === id)
    const confirmed = await confirm({ title: t('linkgrabber.confirm.remove_package_title'), description: t('linkgrabber.confirm.remove_package_description', { name: pkg?.name ?? id }), confirmLabel: t('linkgrabber.actions.delete_package'), confirmIcon: 'i-lucide-trash-2', destructive: true })
    if (confirmed) await collector.deletePackage(id)
  }

  async function removeCandidate(id: string): Promise<void> {
    const candidate = collector.candidates.find(item => item.id === id)
    const confirmed = await confirm({ title: t('linkgrabber.confirm.remove_candidate_title'), description: t('linkgrabber.confirm.remove_candidate_description', { name: candidate?.file_name || candidate?.url || id }), confirmLabel: t('linkgrabber.actions.delete_link'), confirmIcon: 'i-lucide-trash-2', destructive: true })
    if (confirmed) await collector.deleteCandidate(id)
  }

  /**
   * Takes a proposed mirror group apart, after asking once (RD-110-34).
   *
   * It deletes nothing, so it wears neither the destructive styling nor the bin — but it cannot
   * be undone from the list afterwards, because the rows it leaves behind no longer say which
   * group they came from. That is what the question is for, and the description says the links
   * stay and only the grouping goes.
   */
  async function dissolveMirror(id: string): Promise<void> {
    const group = collector.candidates.find(item => item.id === id)?.mirror
    const count = group ? collector.candidates.filter(item => item.mirror?.group === group.group).length : 0
    const confirmed = await confirm({ title: t('linkgrabber.confirm.dissolve_mirror_title'), description: t('linkgrabber.confirm.dissolve_mirror_description', { count }, count), confirmLabel: t('linkgrabber.mirror.dissolve'), confirmIcon: 'i-lucide-ungroup' })
    if (confirmed) await collector.dissolveMirror(id)
  }

  /**
   * "Delete all" clears everything the list shows: collector links (torrents included) plus NZB
   * imports. `r` starts it and answers its question too.
   */
  async function clearAll(): Promise<void> {
    const nzbIds = view.nzbGroups.value.map(entry => entry.id)
    const count = collector.candidates.length + nzbIds.length
    const confirmed = await confirm({ title: t('linkgrabber.confirm.clear_all_title'), description: t('linkgrabber.confirm.clear_all_description', { count }, count), confirmLabel: t('linkgrabber.actions.clear_all'), confirmIcon: 'i-lucide-list-x', destructive: true, confirmKey: 'r' })
    if (!confirmed) return
    if (collector.candidates.length) await collector.clearCandidates()
    for (const id of nzbIds) await nzb.remove(id)
  }

  async function enqueueNzb(id: string, paused = false): Promise<void> {
    if (await nzb.enqueue(id, paused)) await transfers.refresh()
  }

  async function deleteNzb(id: string): Promise<void> {
    const item = nzb.imports.find(entry => entry.id === id)
    const confirmed = await confirm({ title: t('linkgrabber.confirm.remove_nzb_title'), description: t('linkgrabber.confirm.remove_nzb_description', { name: item?.name ?? id }), confirmLabel: t('linkgrabber.actions.delete_nzb'), confirmIcon: 'i-lucide-trash-2', destructive: true })
    if (confirmed) await nzb.remove(id)
  }

  function setNzbCategory(id: string, categoryId: string | null): void {
    void nzb.update(id, { categoryId })
  }

  function setNzbPriority(id: string, priority: DownloadPriority): void {
    void nzb.update(id, { priority })
  }

  function setCategory(ids: string[], categoryId: string | null): void {
    void collector.updatePackages(ids, { categoryId })
  }

  function setPriority(ids: string[], priority: DownloadPriority): void {
    void collector.updatePackages(ids, { priority })
  }

  /**
   * Bulk category and priority fan out over both halves of the selection.
   *
   * The grabber list mixes collector packages and NZB imports, and the selection keeps them in
   * separate id lists. The bulk bar used to read only the collector half, so a pure NZB selection
   * greyed the controls out and a mixed one silently skipped the NZBs.
   */
  async function applyToSelection(change: { categoryId?: string | null, priority?: DownloadPriority }): Promise<void> {
    bulkBusy.value = true
    await Promise.all([
      collector.updatePackages(packagesOf(selection.collectorIds.value), change),
      ...selection.nzbIds.value.map(id => nzb.update(id, change))
    ])
    bulkBusy.value = false
  }

  /** Post-processing is a package setting, so it goes to the packages the selected links are in. */
  function setSelectionPostprocessLevel(level: PostprocessLevel | null): void {
    void collector.updatePackages(packagesOf(selection.collectorIds.value), { postprocessLevel: level })
  }

  function packagesOf(candidateIds: string[]): string[] {
    // A Set, not `.includes()` inside a filter: the bulk bar calls this with the whole selection.
    const wanted = new Set(candidateIds)
    return [...new Set(collector.candidates.filter(c => wanted.has(c.id)).map(c => c.package_id).filter((id): id is string => Boolean(id)))]
  }

  return {
    removeSelected, removeProgress, moveSelected, editPackageDialog, renameCandidate, removePackage, removeCandidate,
    dissolveMirror, clearAll, enqueueNzb, deleteNzb, setNzbCategory, setNzbPriority, setCategory,
    setPriority, applyToSelection, setSelectionPostprocessLevel
  }
}
