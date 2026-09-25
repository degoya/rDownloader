<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Account, Category, Download, DownloadPriority, DownloadSummary, ProxyProfile } from '@/api/types'
import BulkActionBar from '@/components/BulkActionBar.vue'
import DirectAddForm from '@/components/DirectAddForm.vue'
import PackageGroup from '@/components/PackageGroup.vue'
import TransferCard from '@/components/TransferCard.vue'
import VirtualRowList from '@/components/VirtualRowList.vue'
import DataState from '@/components/DataState.vue'
import PostprocessQueue from '@/components/PostprocessQueue.vue'
import QueueSummary from '@/components/QueueSummary.vue'
import PowerCountdownAlert from '@/components/power/PowerCountdownAlert.vue'
import StorageCapacityAlert from '@/components/StorageCapacityAlert.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useOpenSections } from '@/composables/useOpenSections'
import type { VirtualRow } from '@/composables/useVirtualRows'
import { packageEditChange, usePackageEdit } from '@/composables/usePackageEdit'
import { useQueueSelection, type QueueGroup } from '@/composables/useQueueSelection'
import { useRename } from '@/composables/useRename'
import { useResetConfirm } from '@/composables/useResetConfirm'
import { usePostprocessStore } from '@/stores/postprocess'
import { PAUSABLE_STATES, RESETTABLE_STATES, RESUMABLE_STATES, useTransfersStore, type PackageChange } from '@/stores/transfers'
import { hasExtractable } from '@/utils/format'

const { t } = useI18n()
const transfers = useTransfersStore()
const postprocess = usePostprocessStore()
const confirm = useConfirm()
const rename = useRename()
const confirmReset = useResetConfirm()
const editPackage = usePackageEdit()
provide('loadPostprocess', (id: string) => transfers.loadPostprocess(id))

const filter = ref('all')
const adding = ref(false)
const bulkBusy = ref(false)
const draggingId = ref<string | null>(null)
const draggingFileId = ref<string | null>(null)
const categories = ref<Category[]>([])
const accounts = ref<Account[]>([])
const proxies = ref<ProxyProfile[]>([])
const summary = ref<DownloadSummary | null>(null)
const packageControlBusy = ref<Record<string, 'pause' | 'resume' | undefined>>({})
let summaryTimer: ReturnType<typeof setInterval> | null = null
let postprocessTimer: ReturnType<typeof setInterval> | null = null
const addForm = ref<{ reset: () => void } | null>(null)

const filters = computed(() => [
  { label: t('downloads.filters.all'), value: 'all' },
  { label: t('downloads.filters.active'), value: 'active' },
  { label: t('downloads.filters.queued'), value: 'queued' },
  { label: t('downloads.filters.completed'), value: 'completed' }
])
const clearItems = computed(() => [[
  { label: t('downloads.header.clear_completed'), icon: 'i-lucide-circle-check', onSelect: () => clearDownloads('completed') },
  { label: t('downloads.header.clear_failed'), icon: 'i-lucide-file-x-2', onSelect: () => clearDownloads('failed') },
  { label: t('downloads.header.clear_all'), icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => clearDownloads('all') }
]])
const ACTIVE = ['resolving', 'downloading', 'verifying', 'repairing', 'extracting']

const visible = computed(() => {
  if (filter.value === 'all') return transfers.downloads
  if (filter.value === 'active') return transfers.downloads.filter((item) => ACTIVE.includes(item.state))
  return transfers.downloads.filter((item) => item.state === filter.value)
})
// Bucketed in one pass rather than filtering the whole list once per package: that was
// O(packages x downloads) and re-ran on every store refresh, several times a second under load.
const groups = computed<QueueGroup[]>(() => {
  const byPackage = new Map<string, typeof visible.value>()
  for (const item of visible.value) {
    if (!item.package_id) continue
    const bucket = byPackage.get(item.package_id)
    if (bucket) bucket.push(item)
    else byPackage.set(item.package_id, [item])
  }
  return transfers.packages
    .map(pkg => ({ package: pkg, downloads: byPackage.get(pkg.id) ?? [] }))
    .filter(group => group.downloads.length > 0)
})

/**
 * The queue as one stream of rows (RD-106-12).
 *
 * A package header, then its files while it is open. Collapsing is a filter on this stream
 * rather than a `v-if` inside a package, because a virtualized list can only window a flat
 * sequence — and the key of a row has to stay the same when the row moves, so that the focus
 * and the selection survive the window sliding past it.
 */
interface QueueRowBase extends VirtualRow { group: QueueGroup }
interface QueuePackageRow extends QueueRowBase { kind: 'package' }
interface QueueFileRow extends QueueRowBase { kind: 'file', download: Download }
type QueueRow = QueuePackageRow | QueueFileRow

/** Starting estimates only; the list measures what the rows really are once they are drawn. */
const PACKAGE_ROW_SIZE = 52
const FILE_ROW_SIZE = 40

const openPackages = useOpenSections({ storageKey: 'rdownloader-open-packages', defaultOpen: false })

const rows = computed<QueueRow[]>(() => {
  const result: QueueRow[] = []
  for (const group of groups.value) {
    result.push({ key: `package:${group.package.id}`, size: PACKAGE_ROW_SIZE, class: 'pt-2', kind: 'package', group })
    if (!openPackages.isOpen(group.package.id)) continue
    for (const download of group.downloads) {
      result.push({ key: `file:${download.id}`, size: FILE_ROW_SIZE, kind: 'file', group, download })
    }
  }
  return result
})
/** The order a range selection follows: what is on screen, not what is in the store. */
const orderedFileIds = computed(() => rows.value.flatMap(row => row.kind === 'file' ? [row.download.id] : []))
const selection = useQueueSelection(groups, computed(() => transfers.downloads), orderedFileIds)

const queueList = ref<{
  focusRow: (key: string) => Promise<boolean>
  revealRow: (key: string) => Promise<boolean>
} | null>(null)
/**
 * Whether the pick that is about to arrive is holding shift.
 *
 * A checkbox reports `update:modelValue`, not the event that caused it, and the range has to
 * know about the modifier. The capture-phase click on the list runs first, so the flag is set
 * by the time the checkbox reports.
 */
const extendSelection = ref(false)
function noteModifier(event: MouseEvent | KeyboardEvent): void {
  extendSelection.value = event.shiftKey
}

/** Border frame of a file row: the package's frame carried down its children. */
function fileFrame(group: QueueGroup): string {
  return `border-x border-b ${selection.packageState(group) !== 'none' ? 'border-primary' : 'border-muted'}`
}

/**
 * Jumps to the first selected file and puts the keyboard on it.
 *
 * It may sit in a collapsed package, or a thousand rows below the window — both are why this
 * exists at all: without it a selection made through the header checkbox has no way back to
 * the row it belongs to.
 */
async function revealSelection(): Promise<void> {
  const [id] = selection.selectedIds.value
  if (!id) return
  const download = transfers.downloads.find(item => item.id === id)
  if (download?.package_id && !openPackages.isOpen(download.package_id)) {
    openPackages.set(download.package_id, true)
    await nextTick()
  }
  await queueList.value?.focusRow(`file:${id}`)
}
const stateFingerprint = computed(() => transfers.downloads.map(download => `${download.id}:${download.state}`).join('|'))
const packageStateFingerprint = computed(() => transfers.packages.map(pkg => `${pkg.id}:${pkg.state ?? ''}`).join('|'))
const canExtractSelection = computed(() => hasExtractable(selection.selectedDownloads.value))
const resettableSelection = computed(() => selection.selectedDownloads.value.filter(download => RESETTABLE_STATES.includes(download.state)))

async function copyPath(path: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(path)
    transfers.notice = t('downloads.notices.path_copied', { path })
  } catch {
    transfers.notice = t('downloads.notices.destination', { path })
  }
}

/**
 * States that make a package removal destructive: cancelling these loses what they had already
 * written. The server refuses such a package unless the request says `force`, so the dialog has
 * to name the cost before the flag is sent (RD-107-07).
 */
const BUSY_STATES = [...ACTIVE, 'queued', 'retry_wait', 'paused', 'seeding']

function busyPackageCount(ids: string[]): number {
  return ids.filter(id => transfers.downloads.some(item => item.package_id === id && BUSY_STATES.includes(item.state))).length
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

onMounted(() => {
  void loadSelections()
  void loadSummary()
  void postprocess.refresh()
  // The summary follows the SSE-driven store refresh (see the fingerprint watchers);
  // this is only a long safety net for storage figures and missed events.
  summaryTimer = setInterval(() => void loadSummary(), 120_000)
  // Safety net for the pipeline queue (missed events, pending → running transitions). Kept
  // deliberately slow: `postprocess.progress` events and the fingerprint watcher below already
  // drive this, and a 3s poll competed with real traffic for the browser's connection budget.
  postprocessTimer = setInterval(() => { if (postprocess.active) void postprocess.refresh() }, 15_000)
})
onUnmounted(() => {
  if (summaryTimer) clearInterval(summaryTimer)
  if (postprocessTimer) clearInterval(postprocessTimer)
})
// `download.state` / `package.state` events feed the store's debounced refresh (400 ms);
// both fingerprints change with it, which keeps the summary reactive without a manual button.
watch(stateFingerprint, () => void loadSummary())
watch(packageStateFingerprint, () => {
  void loadSummary()
  void postprocess.refresh()
})

async function loadSummary(): Promise<void> {
  const response = await api.GET('/api/v1/downloads/summary')
  if (response.data) summary.value = response.data
}

async function loadSelections(): Promise<void> {
  const [categoryResponse, accountResponse, proxyResponse] = await Promise.all([
    api.GET('/api/v1/categories'),
    api.GET('/api/v1/accounts'),
    api.GET('/api/v1/proxy-profiles')
  ])
  if (categoryResponse.data) categories.value = categoryResponse.data
  if (accountResponse.data) accounts.value = accountResponse.data
  if (proxyResponse.data) proxies.value = proxyResponse.data
}

function accountLabel(id: string | null | undefined): string | null {
  if (!id) return null
  const account = accounts.value.find(item => item.id === id)
  return account ? `${account.label} (${account.provider})` : null
}

function togglePackage(id: string, selected: boolean): void {
  const group = groups.value.find(item => item.package.id === id)
  if (group) selection.togglePackage(group, selected)
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
  return transfers.downloads.some(download => download.package_id === id && states.includes(download.state))
}

async function controlPackage(id: string, action: 'pause' | 'resume'): Promise<void> {
  if (packageControlBusy.value[id]) return
  const states = action === 'pause' ? PAUSABLE_STATES : RESUMABLE_STATES
  const ids = transfers.downloads
    .filter(download => download.package_id === id && states.includes(download.state))
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
  const targets = transfers.downloads.filter(download => ids.includes(download.id) && RESETTABLE_STATES.includes(download.state))
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

async function renamePackage(id: string): Promise<void> {
  const pkg = transfers.packages.find(item => item.id === id)
  if (!pkg) return
  const result = await editPackage({ name: pkg.name, hasPassword: pkg.has_password ?? false, password: pkg.password ?? null, postprocessLevel: pkg.postprocess_level ?? null, script: pkg.script ?? null, canRenameFolder: true })
  if (!result) return
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

async function onDrop(targetId: string): Promise<void> {
  // A file dropped on a package header: moving files between packages is not a gesture this
  // list offers, so the drag ends here rather than doing something the user did not ask for.
  if (draggingFileId.value) {
    draggingFileId.value = null
    return
  }
  const sourceId = draggingId.value
  draggingId.value = null
  if (!sourceId || sourceId === targetId) return
  const source = transfers.packages.find(item => item.id === sourceId)
  const target = transfers.packages.find(item => item.id === targetId)
  if (!source || !target) return
  if (source.priority !== target.priority) {
    transfers.notice = t('downloads.notices.reorder_same_priority')
    return
  }
  const order = transfers.packages.map(item => item.id).filter(id => id !== sourceId)
  order.splice(order.indexOf(targetId), 0, sourceId)
  await transfers.reorderPackages(order)
}

/**
 * Writes the file order of one package after a drag or a keyboard move.
 *
 * A filtered list shows only part of the package, and the endpoint takes the complete id list,
 * so the filter case says why instead of dropping the gesture silently.
 */
async function persistFileOrder(packageId: string, order: string[]): Promise<boolean> {
  if (filter.value !== 'all') {
    transfers.notice = t('downloads.notices.reorder_filter_active')
    return false
  }
  transfers.notice = null
  await transfers.reorderDownloads(packageId, order)
  return true
}

/** File dropped on another file: both have to sit in the same package. */
async function onFileDrop(targetId: string): Promise<void> {
  const sourceId = draggingFileId.value
  draggingFileId.value = null
  if (!sourceId || sourceId === targetId) return
  const source = transfers.downloads.find(item => item.id === sourceId)
  const target = transfers.downloads.find(item => item.id === targetId)
  if (!source || !target || !target.package_id || source.package_id !== target.package_id) return
  const group = groups.value.find(entry => entry.package.id === target.package_id)
  if (!group) return
  const order = group.downloads.map(item => item.id).filter(id => id !== sourceId)
  order.splice(order.indexOf(targetId), 0, sourceId)
  await persistFileOrder(target.package_id, order)
}

/** Keyboard counterpart of the file drag: one step up or down inside the package. */
async function onFileMove(id: string, delta: -1 | 1): Promise<void> {
  const download = transfers.downloads.find(item => item.id === id)
  if (!download?.package_id) return
  const group = groups.value.find(entry => entry.package.id === download.package_id)
  if (!group) return
  const order = group.downloads.map(item => item.id)
  const from = order.indexOf(id)
  const to = from + delta
  if (from < 0 || to < 0 || to >= order.length) return
  order.splice(to, 0, ...order.splice(from, 1))
  // The row has moved, and with a windowed list its new place may be outside what is rendered.
  // Focus is put back on the same handle so a second press continues the move (RD-106-12).
  if (await persistFileOrder(download.package_id, order)) await queueList.value?.focusRow(`file:${id}`)
}

/** Keyboard counterpart of the package drag; the priority tier bounds it just as the drag does. */
async function onPackageMove(id: string, delta: -1 | 1): Promise<void> {
  const order = transfers.packages.map(item => item.id)
  const from = order.indexOf(id)
  const to = from + delta
  if (from < 0 || to < 0 || to >= order.length) return
  const source = transfers.packages[from]
  const neighbour = transfers.packages[to]
  if (!source || !neighbour) return
  if (source.priority !== neighbour.priority) {
    transfers.notice = t('downloads.notices.reorder_same_priority')
    return
  }
  transfers.notice = null
  order.splice(to, 0, ...order.splice(from, 1))
  await transfers.reorderPackages(order)
  await queueList.value?.focusRow(`package:${id}`)
}

async function addDownload(payload: { url: string, categoryId?: string, accountId?: string, proxyProfileId?: string, priority: DownloadPriority }): Promise<void> {
  adding.value = true
  const ok = await transfers.add(payload.url, undefined, undefined, {
    ...(payload.categoryId ? { categoryId: payload.categoryId } : {}),
    ...(payload.accountId ? { accountId: payload.accountId } : {}),
    ...(payload.proxyProfileId ? { proxyProfileId: payload.proxyProfileId } : {}),
    priority: payload.priority
  })
  if (ok) addForm.value?.reset()
  adding.value = false
}

async function clearDownloads(scope: 'completed' | 'failed' | 'all'): Promise<void> {
  const confirmed = await confirm({ title: t('downloads.confirm.clear_title'), description: t(`downloads.confirm.clear_${scope}`), confirmLabel: t('downloads.confirm.clear_label'), confirmIcon: 'i-lucide-trash-2', destructive: true })
  if (confirmed) await transfers.clear(scope)
}

async function removeDownload(id: string): Promise<void> {
  const download = transfers.downloads.find(item => item.id === id)
  const description = t(download?.state === 'completed' ? 'downloads.confirm.remove_download_completed' : 'downloads.confirm.remove_download_partial')
  const confirmed = await confirm({ title: t('downloads.confirm.remove_download_title'), description, confirmLabel: t('downloads.confirm.remove_download_title'), confirmIcon: 'i-lucide-trash-2', destructive: true })
  if (confirmed) await transfers.remove(id)
}
</script>

<template>
  <UDashboardPanel id="downloads">
    <template #header>
      <UDashboardNavbar :title="t('downloads.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
        <template #right>
          <div data-tour="downloads-controls" class="flex items-center gap-2">
          <UBadge color="neutral" variant="outline" class="font-mono">{{ t('common.units.file', { count: transfers.downloads.length }, transfers.downloads.length).toLocaleUpperCase() }}</UBadge>
          <UButton
            v-if="transfers.globalControl"
            :icon="transfers.globalControl === 'pause' ? 'i-lucide-pause' : 'i-lucide-play'"
            :label="transfers.globalControl === 'pause' ? t('downloads.header.pause_all') : t('downloads.header.resume_all')"
            :color="transfers.globalControl === 'pause' ? 'neutral' : 'primary'"
            :variant="transfers.globalControl === 'pause' ? 'outline' : 'soft'"
            :loading="transfers.controlsBusy"
            @click="transfers.controlAll(transfers.globalControl)"
          />
          <UDropdownMenu :items="clearItems">
            <UButton icon="i-lucide-list-x" :label="t('downloads.header.clear_list')" color="neutral" variant="outline" :loading="transfers.clearing" />
          </UDropdownMenu>
          </div>
        </template>
      </UDashboardNavbar>
      <UDashboardToolbar>
        <template #left>
          <USelect v-model="filter" :items="filters" value-key="value" class="w-36" :aria-label="t('downloads.filters.aria')" />
          <UCheckbox
            :model-value="selection.state.value === 'all' ? true : selection.state.value === 'some' ? 'indeterminate' : false"
            :disabled="!groups.length"
            :label="selection.count.value ? t('downloads.header.selection_count', { count: selection.count.value }) : t('common.actions.select_all')"
            :aria-label="t('downloads.header.select_all_hint')"
            @update:model-value="selection.toggleAll()"
          />
        </template>
        <template #right>
          <span class="numeric text-xs text-muted">{{ t('common.units.package', { count: groups.length }, groups.length) }} · {{ t('common.units.file', { count: visible.length }, visible.length) }}</span>
        </template>
      </UDashboardToolbar>
    </template>

    <template #body>
      <div class="flex w-full flex-col gap-6">
        <div data-tour="downloads-add">
          <DirectAddForm ref="addForm" :categories="categories" :accounts="accounts" :proxies="proxies" :busy="adding" @submit="addDownload" />
        </div>

        <PowerCountdownAlert />

        <StorageCapacityAlert />

        <UAlert v-if="transfers.error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="transfers.error" />
        <UAlert v-if="transfers.notice" color="info" variant="subtle" icon="i-lucide-info" :description="transfers.notice" />

        <PostprocessQueue v-if="postprocess.queue.length" :entries="postprocess.queue" />

        <section v-if="rows.length" class="space-y-2">
          <BulkActionBar
            v-if="selection.selectedIds.value.length"
            :count="selection.selectedIds.value.length"
            unit-key="common.units.file"
            :categories="categories"
            :busy="bulkBusy"
            :package-actions-disabled="!selection.fullySelectedPackageIds.value.length"
            :can-extract="canExtractSelection"
            transfer-actions
            @category="(id) => changePackages(selection.fullySelectedPackageIds.value, { categoryId: id })"
            @priority="(value) => changePackages(selection.fullySelectedPackageIds.value, { priority: value })"
            @postprocess="(level) => changePackages(selection.fullySelectedPackageIds.value, { postprocessLevel: level })"
            @resume="bulkAction('resume')"
            @pause="bulkAction('pause')"
            @cancel="bulkAction('cancel')"
            @extract="bulkExtract"
            @remove="bulkRemove"
            @clear="selection.clear()"
          >
            <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-crosshair" :label="t('common.actions.reveal')" @click="revealSelection" />
            <UButton v-if="selection.selectedIds.value.length === 1" size="sm" color="neutral" variant="outline" icon="i-lucide-pencil" :label="t('common.actions.rename')" @click="bulkRename" />
            <UButton v-if="resettableSelection.length" size="sm" color="error" variant="outline" icon="i-lucide-rotate-ccw" :label="t('downloads.bulk.reset', { count: resettableSelection.length }, resettableSelection.length)" :loading="bulkBusy" @click="resetDownloads(selection.selectedIds.value)" />
            <UButton v-if="selection.fullySelectedPackageIds.value.length" size="sm" color="error" variant="outline" icon="i-lucide-package-x" :label="t('downloads.confirm.delete_packages_label', { count: selection.fullySelectedPackageIds.value.length }, selection.fullySelectedPackageIds.value.length)" :loading="bulkBusy" @click="bulkDeletePackages" />
          </BulkActionBar>
          <!--
            One flattened stream of rows through the shared list block: package headers and the
            files of the open ones, keyed so a row keeps its identity while the window slides.
            The capture-phase handlers read the shift key before the checkbox reports its new
            value, which is what turns a pick into a range (RD-106-12).
          -->
          <VirtualRowList
            ref="queueList"
            :rows="rows"
            :label="t('downloads.list.aria', { count: rows.length })"
            @click.capture="noteModifier"
            @keydown.capture="noteModifier"
          >
            <template #row="{ row }">
              <PackageGroup
                v-if="row.kind === 'package'"
                :package="row.group.package"
                :downloads="row.group.downloads"
                :categories="categories"
                :selection="selection.packageState(row.group)"
                :open="openPackages.isOpen(row.group.package.id)"
                :complete="transfers.packageComplete[row.group.package.id] ?? false"
                :package-rate="transfers.packageRates[row.group.package.id] ?? 0"
                :package-eta="transfers.packageEtas[row.group.package.id] ?? null"
                :dragging="draggingId === row.group.package.id"
                :can-pause="canControlPackage(row.group.package.id, 'pause')"
                :can-resume="canControlPackage(row.group.package.id, 'resume')"
                :control-busy="packageControlBusy[row.group.package.id] ?? null"
                @select="togglePackage"
                @toggle="openPackages.toggle"
                @category="(id, categoryId) => changePackages([id], { categoryId })"
                @priority="(id, value) => changePackages([id], { priority: value })"
                @rename="renamePackage"
                @extract="(id) => extractPackages([id])"
                @force-extract="forceExtractPackage"
                @dragstart="(id) => draggingId = id"
                @drop="onDrop"
                @move="onPackageMove"
                @pause-package="(id) => controlPackage(id, 'pause')"
                @resume-package="(id) => controlPackage(id, 'resume')"
                @delete-package="deletePackage"
                @copy-path="copyPath"
              />
              <TransferCard
                v-else
                :class="fileFrame(row.group)"
                :download="row.download"
                :bytes-per-second="transfers.downloadRates[row.download.id] ?? 0"
                :eta-seconds="transfers.downloadEtas[row.download.id] ?? null"
                :destination="row.group.package.destination"
                :account-label="accountLabel(row.download.account_id)"
                :selected="selection.selectedFiles.value.has(row.download.id)"
                @select="(id, value) => selection.pickFile(id, value, extendSelection)"
                @pause="(id) => transfers.control(id, 'pause')"
                @resume="(id) => transfers.control(id, 'resume')"
                @cancel="(id) => transfers.control(id, 'cancel')"
                @stop-seeding="(id) => transfers.control(id, 'stop_seeding')"
                @remove="removeDownload"
                @reset="(id) => resetDownloads([id])"
                @rename="renameFile"
                @copy-path="copyPath"
                @dragstart="(id) => draggingFileId = id"
                @drop="onFileDrop"
                @move="onFileMove"
              />
            </template>
          </VirtualRowList>
        </section>

        <!--
          "Nothing here" is only true once the queue fetch has settled. Until then the store's
          own loading flag is what the reader sees, and a failed fetch stays visible as the
          error alert above rather than dissolving into an empty queue (RD-104-07).
        -->
        <DataState v-else :loading="transfers.loading" :empty="!transfers.error" :rows="4">
          <section class="signal-grid grid min-h-72 place-items-center border border-dashed border-muted p-8 text-center">
            <div>
              <UIcon name="i-lucide-inbox" class="mx-auto mb-4 size-8 text-muted" />
              <h2 class="font-medium text-highlighted">{{ t('downloads.empty.title') }}</h2>
              <p class="mt-2 text-sm text-muted">{{ t('downloads.empty.hint') }}</p>
            </div>
          </section>
        </DataState>

        <QueueSummary
          v-if="summary"
          :summary="summary"
          :current-rate="transfers.globalRate"
          :eta-seconds="transfers.queueEta"
          :speed-history="transfers.speedHistory"
        />
      </div>
    </template>
  </UDashboardPanel>
</template>
