import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Download } from '@/api/types'
import { useClearEverythingConfirm } from '@/composables/useClearEverythingConfirm'
import { useConfirm } from '@/composables/useConfirm'
import { copyText } from '@/composables/useCopy'
import { useCopyLinks } from '@/composables/useCopyLinks'
import { packageEditChange, usePackageEdit } from '@/composables/usePackageEdit'
import { usePackageStorage } from '@/composables/usePackageStorage'
import type { QueueGroup, useQueueSelection } from '@/composables/useQueueSelection'
import { useRename } from '@/composables/useRename'
import { useResetConfirm } from '@/composables/useResetConfirm'
import { PAUSABLE_STATES, PENDING_STATES, RESETTABLE_STATES, RESUMABLE_STATES, useTransfersStore, type PackageChange } from '@/stores/transfers'
import { MIB } from '@/utils/format'

const ACTIVE = ['resolving', 'downloading', 'verifying', 'repairing', 'extracting']

/** Whether two limits in MiB/s are the same once stored as whole bytes per second. */
function sameSpeedLimit(left: number | null, right: number | null): boolean {
  return Math.round((left ?? 0) * MIB) === Math.round((right ?? 0) * MIB)
}

/**
 * States that make a package removal destructive: cancelling these loses what they had already
 * written. The server refuses such a package unless the request says `force`, so the dialog has
 * to name the cost before the flag is sent (RD-107-07).
 */
const BUSY_STATES = [...ACTIVE, 'queued', 'retry_wait', 'paused', 'seeding']

/** The states the server's "still working" refusal reads, so the count matches what stops. */
const WORKING_STATES: readonly string[] = [...PENDING_STATES, 'seeding']

/**
 * What the Downloads view does to files and packages: the confirmations, the bulk actions, the
 * package menu, renaming and clearing the list — split out of `DownloadsView` (WEB-13).
 */
export function useDownloadsActions(view: {
  selection: ReturnType<typeof useQueueSelection>
  /** The packages as displayed, with their filtered files. */
  groups: Ref<QueueGroup[]>
  /** Every file of a package, whatever the filter hides. */
  packageDownloads: (id: string) => Download[]
}) {
  const { t } = useI18n()
  const transfers = useTransfersStore()
  const confirm = useConfirm()
  const rename = useRename()
  const confirmReset = useResetConfirm()
  const confirmClearEverything = useClearEverythingConfirm()
  const editPackage = usePackageEdit()
  const openPackageStorage = usePackageStorage()
  const copyLinks = useCopyLinks()
  const { selection, groups, packageDownloads } = view

  const bulkBusy = ref(false)
  const packageControlBusy = ref<Record<string, 'pause' | 'resume' | undefined>>({})

  /** Every file's address of the package, whatever the filter hides: the package is what was asked for. */
  function copyPackageLinks(id: string): void {
    void copyLinks(transfers.downloads.filter(download => download.package_id === id).map(download => download.source))
  }

  async function copyPath(path: string): Promise<void> {
    // No toast on failure: the notice names the path, which can be copied from there by hand.
    transfers.notice = await copyText(path)
      ? t('downloads.notices.path_copied', { path })
      : t('downloads.notices.destination', { path })
  }

  function busyPackageCount(ids: string[]): number {
    return ids.filter(id => packageDownloads(id).some(item => BUSY_STATES.includes(item.state))).length
  }

  async function deletePackage(id: string): Promise<void> {
    const pkg = transfers.packages.find(item => item.id === id)
    if (!pkg) return
    const busy = busyPackageCount([id]) > 0
    const confirmed = await confirm({
      title: t('downloads.confirm.delete_package_title'),
      description: t(busy ? 'downloads.confirm.delete_package_active' : 'downloads.confirm.delete_package_description', { name: pkg.name }),
      confirmLabel: t('downloads.confirm.delete_package_title'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (!confirmed) return
    bulkBusy.value = true
    await transfers.deletePackages([id], busy)
    bulkBusy.value = false
  }

  async function bulkDeletePackages(): Promise<void> {
    const ids = selection.fullySelectedPackageIds.value
    if (!ids.length) return
    const busy = busyPackageCount(ids)
    const confirmed = await confirm({
      title: t('downloads.confirm.delete_packages_title'),
      description: busy
        ? t('downloads.confirm.delete_packages_active', { count: ids.length, busy })
        : t('downloads.confirm.delete_packages_description', { count: ids.length }, ids.length),
      confirmLabel: t('downloads.confirm.delete_packages_title'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (!confirmed) return
    bulkBusy.value = true
    await transfers.deletePackages(ids, busy > 0)
    selection.clear()
    bulkBusy.value = false
  }

  async function changePackages(ids: string[], change: PackageChange): Promise<void> {
    if (!ids.length) return
    bulkBusy.value = true
    await transfers.updatePackages(ids, change)
    bulkBusy.value = false
  }

  async function bulkAction(action: 'pause' | 'resume' | 'cancel'): Promise<void> {
    bulkBusy.value = true
    await transfers.bulk(selection.selectedIds.value, action)
    bulkBusy.value = false
  }

  function canControlPackage(id: string, action: 'pause' | 'resume'): boolean {
    const states = action === 'pause' ? PAUSABLE_STATES : RESUMABLE_STATES
    return packageDownloads(id).some(download => states.includes(download.state))
  }

  async function controlPackage(id: string, action: 'pause' | 'resume'): Promise<void> {
    if (packageControlBusy.value[id]) return
    const states = action === 'pause' ? PAUSABLE_STATES : RESUMABLE_STATES
    const ids = packageDownloads(id)
      .filter(download => states.includes(download.state))
      .map(download => download.id)
    if (!ids.length) return

    packageControlBusy.value = { ...packageControlBusy.value, [id]: action }
    try {
      await transfers.bulk(ids, action)
    } finally {
      const remaining = { ...packageControlBusy.value }
      delete remaining[id]
      packageControlBusy.value = remaining
    }
  }

  async function bulkRemove(): Promise<void> {
    const count = selection.selectedIds.value.length
    const confirmed = await confirm({
      title: t('downloads.confirm.remove_files_title'),
      description: t('downloads.confirm.remove_files_description', { count }, count),
      confirmLabel: t('common.actions.remove'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (!confirmed) return
    bulkBusy.value = true
    await transfers.bulk(selection.selectedIds.value, 'remove')
    selection.clear()
    bulkBusy.value = false
  }

  /**
   * Resets the given files after confirming what that costs.
   *
   * Only files that are not moving are offered; an active one would have to be paused first, and
   * silently skipping it reads as the action having worked.
   */
  async function resetDownloads(ids: string[]): Promise<void> {
    const wanted = new Set(ids)
    const targets = transfers.downloads.filter(download => wanted.has(download.id) && RESETTABLE_STATES.includes(download.state))
    if (!targets.length) {
      transfers.notice = t('downloads.notices.nothing_to_reset')
      return
    }
    const result = await confirmReset(
      targets.map(download => download.file_name),
      targets.some(download => download.state === 'completed' || download.state === 'seeding')
    )
    if (!result.confirmed) return
    bulkBusy.value = true
    await transfers.reset(targets.map(download => download.id), result.deleteFiles)
    selection.clear()
    bulkBusy.value = false
  }

  async function bulkExtract(): Promise<void> {
    bulkBusy.value = true
    await transfers.extractDownloads(selection.selectedIds.value)
    bulkBusy.value = false
  }

  async function bulkRename(): Promise<void> {
    const [id] = selection.selectedIds.value
    if (id) await renameFile(id)
  }

  /**
   * The package's collision policy and its files' duplicates (RD-150-01). Every file of the
   * package is offered, whatever the view's filter hides.
   */
  async function packageStorage(id: string): Promise<void> {
    const group = groups.value.find(entry => entry.package.id === id)
    if (!group) return
    await openPackageStorage({
      packageId: id,
      packageName: group.package.name,
      downloads: transfers.downloads
        .filter(item => item.package_id === id)
        .map(item => ({ id: item.id, file_name: item.file_name, state: item.state }))
    })
  }

  async function renamePackage(id: string): Promise<void> {
    const pkg = transfers.packages.find(item => item.id === id)
    if (!pkg) return
    // The package's own speed limit (RD-1100-01); left out of the editor when it cannot be read.
    const limit = await transfers.loadPackageSpeedLimit(id)
    const result = await editPackage({
      name: pkg.name,
      hasPassword: pkg.has_password ?? false,
      password: pkg.password ?? null,
      postprocessLevel: pkg.postprocess_level ?? null,
      script: pkg.script ?? null,
      canRenameFolder: true,
      ...(limit ? { speedLimitMiB: limit.mib, speedLimitSupported: limit.supported } : {})
    })
    if (!result) return
    if (result.speedLimitMiB !== undefined && limit && !sameSpeedLimit(result.speedLimitMiB, limit.mib)) {
      await transfers.setPackageSpeedLimit(id, result.speedLimitMiB)
    }
    const change = packageEditChange(pkg, result)
    // Renaming the folder carries the name with it, so the label must not be sent twice: the
    // second request would find the package already renamed and report no change at all.
    if (result.renameFolder) delete change.name
    if (Object.keys(change).length) await transfers.updatePackages([id], change)
    if (result.renameFolder) await transfers.renamePackageFolder(id, result.name)
  }

  async function extractPackages(ids: string[]): Promise<void> {
    bulkBusy.value = true
    await transfers.extractPackages(ids)
    bulkBusy.value = false
  }

  /** Runs the pipeline for a package whose verification failed, this once (RD-104-04). */
  async function forceExtractPackage(id: string): Promise<void> {
    bulkBusy.value = true
    await transfers.forceExtractPackage(id)
    bulkBusy.value = false
  }

  async function renameFile(id: string): Promise<void> {
    const download = transfers.downloads.find(item => item.id === id)
    if (!download) return
    const name = await rename({ title: t('downloads.rename_file.title'), label: t('downloads.rename_file.label'), value: download.file_name, description: t('downloads.rename_file.description') })
    if (name) await transfers.renameDownload(id, name)
  }

  async function clearDownloads(scope: 'completed' | 'failed' | 'all'): Promise<void> {
    // `k` opens the question for the completed packages, and `k` again answers it (RD-180-17).
    const confirmKey = scope === 'completed' ? 'k' : undefined
    const confirmed = await confirm({ title: t('downloads.confirm.clear_title'), description: t(`downloads.confirm.clear_${scope}`), confirmLabel: t('downloads.confirm.clear_label'), confirmIcon: 'i-lucide-trash-2', destructive: true, confirmKey })
    if (confirmed) await transfers.clear(scope)
  }

  async function clearEverything(): Promise<void> {
    const working = new Set(transfers.downloads.filter(download => WORKING_STATES.includes(download.state)).map(download => download.package_id))
    const answer = await confirmClearEverything(transfers.packages.length, working.size)
    if (answer.confirmed) await transfers.clear('everything', answer.deletePartial)
  }

  async function removeDownload(id: string): Promise<void> {
    const download = transfers.downloads.find(item => item.id === id)
    const description = t(download?.state === 'completed' ? 'downloads.confirm.remove_download_completed' : 'downloads.confirm.remove_download_partial')
    const confirmed = await confirm({ title: t('downloads.confirm.remove_download_title'), description, confirmLabel: t('downloads.confirm.remove_download_title'), confirmIcon: 'i-lucide-trash-2', destructive: true })
    if (confirmed) await transfers.remove(id)
  }

  return {
    bulkBusy,
    packageControlBusy,
    copyLinks,
    copyPackageLinks,
    copyPath,
    deletePackage,
    bulkDeletePackages,
    changePackages,
    bulkAction,
    canControlPackage,
    controlPackage,
    bulkRemove,
    resetDownloads,
    bulkExtract,
    bulkRename,
    packageStorage,
    renamePackage,
    extractPackages,
    forceExtractPackage,
    renameFile,
    clearDownloads,
    clearEverything,
    removeDownload
  }
}
