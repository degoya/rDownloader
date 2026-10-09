<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { DownloadPriority, DownloadSummary } from '@/api/types'
import BulkActionBar from '@/components/BulkActionBar.vue'
import DirectAddForm from '@/components/DirectAddForm.vue'
import PackageGroup from '@/components/PackageGroup.vue'
import QueueColumnHeader from '@/components/QueueColumnHeader.vue'
import QueueResetFailedMenu from '@/components/QueueResetFailedMenu.vue'
import QueueSortNotice from '@/components/QueueSortNotice.vue'
import TransferCard from '@/components/TransferCard.vue'
import VirtualRowList from '@/components/VirtualRowList.vue'
import DataState from '@/components/DataState.vue'
import PostprocessQueue from '@/components/PostprocessQueue.vue'
import QueuePauseControl from '@/components/QueuePauseControl.vue'
import QueueSummary from '@/components/QueueSummary.vue'
import PowerCountdownAlert from '@/components/power/PowerCountdownAlert.vue'
import StorageCapacityAlert from '@/components/StorageCapacityAlert.vue'
import TorrentKillSwitchAlert from '@/components/TorrentKillSwitchAlert.vue'
import CollisionPromptsAlert from '@/components/storage/CollisionPromptsAlert.vue'
import { setIndexerSearchFocusAction } from '@/composables/indexerSearchFocus'
import { setClearCompletedAction } from '@/composables/shortcutDefinitions'
import { useDownloadsActions } from '@/composables/useDownloadsActions'
import { useNzbHandOver } from '@/composables/useNzbHandOver'
import { usePackageExport } from '@/composables/usePackageExport'
import { useReresolve } from '@/composables/useReresolve'
import { usePackageOpenState } from '@/composables/usePackageOpenState'
import { DEFAULT_THRESHOLD } from '@/composables/useVirtualRows'
import { useQueueColumns } from '@/composables/useQueueColumns'
import { filterQueue, QUEUE_FILTERS, useQueueFilter } from '@/composables/useQueueFilter'
import { useQueueReorder } from '@/composables/useQueueReorder'
import { useQueueRows } from '@/composables/useQueueRows'
import { useQueueSort } from '@/composables/useQueueSort'
import { useResetFailed } from '@/composables/useResetFailed'
import { packageRowKey, useQueueSelection, type QueueGroup } from '@/composables/useQueueSelection'
import { useShowMetadata } from '@/composables/useShowMetadata'
import { useAccounts } from '@/stores/accounts'
import { useCategories } from '@/stores/categories'
import { usePostprocessStore } from '@/stores/postprocess'
import { useProxyProfiles } from '@/stores/proxyProfiles'
import { usePublishedSelection } from '@/stores/selection'
import { RESETTABLE_STATES, useTransfersStore } from '@/stores/transfers'
import { hasExtractable } from '@/utils/format'

const { t } = useI18n()
const transfers = useTransfersStore()
const postprocess = usePostprocessStore()
// The NZB behind a package to a remote-job provider, in any state of the package (RD-191-13).
const nzbHandOver = useNzbHandOver('downloads')
// The package as a link file, and resolving again with the plugin installed now (RD-1210-01).
const { exportPackages } = usePackageExport()
const { reresolve } = useReresolve()
provide('loadPostprocess', (id: string) => transfers.loadPostprocess(id))

/** The state filter and the name search, both in the address (RD-190-21). */
const { filter, search, needle, active: filterActive, reset: resetFilter } = useQueueFilter()
const searchField = ref<HTMLElement | null>(null)
const adding = ref(false)
const { categories, fetchCategories } = useCategories()
const { accounts, fetchAccounts } = useAccounts()
const { proxies, fetchProxies } = useProxyProfiles()
const summary = ref<DownloadSummary | null>(null)
let summaryTimer: ReturnType<typeof setInterval> | null = null
let postprocessTimer: ReturnType<typeof setInterval> | null = null
const addForm = ref<{ reset: () => void } | null>(null)

const filters = computed(() => QUEUE_FILTERS.map(value => ({ label: t(`downloads.filters.${value}`), value })))
// One red entry, apart from the rest: the only one that stops work in progress (RD-180-21).
const clearItems = computed(() => [[
  { label: t('downloads.header.clear_completed'), icon: 'i-lucide-circle-check', kbds: ['k'], onSelect: () => clearDownloads('completed') },
  { label: t('downloads.header.clear_failed'), icon: 'i-lucide-file-x-2', onSelect: () => clearDownloads('failed') },
  { label: t('downloads.header.clear_all'), icon: 'i-lucide-list-checks', onSelect: () => clearDownloads('all') }
], [
  { label: t('downloads.header.clear_everything'), icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => clearEverything() }
]])

const visible = computed(() => filterQueue(transfers.downloads, transfers.packages, filter.value, needle.value))

/** Which packages are open, remembered per browser; "all" is what the filter shows (RD-1170-01). */
const openPackages = usePackageOpenState('downloads', {
  known: () => transfers.packages.map(pkg => pkg.id),
  shown: () => groups.value.map(group => group.package.id)
})
/** The "Show metadata" switch: the enricher chips under the package names, per browser (RD-150-19). */
const showMetadata = useShowMetadata('downloads')

/** A sort for the eye only, by a column header; the queue keeps its order (RD-1190-16). */
const queueSort = useQueueSort(id => categories.value.find(category => category.id === id)?.name ?? '')
/** Packages, the flat row stream of the virtualized list, and every file of a package (RD-106-12). */
const { groups, rows, packageDownloads } = useQueueRows(visible, openPackages.isOpen, queueSort.arrange)
/** Every failed or blocked file of the shown list, or of one package, back to the queue (RD-1190-15). */
const resetFailed = useResetFailed({ visible, needle })
/**
 * The order a range selection follows: what is on screen, not what is in the store. A package
 * row is a stop of its own, so a range can run from package to package (RD-170-13).
 */
const orderedRowKeys = computed(() => rows.value.map(row => row.kind === 'file' ? row.download.id : packageRowKey(row.group.package.id)))
const selection = useQueueSelection(groups, computed(() => transfers.downloads), orderedRowKeys)
// How much is ticked, shown in the status bar while this view is open (RD-170-14).
usePublishedSelection(selection.size)

/** The data columns' widths, set on the container of the header row and the rows (RD-191-11). */
const columns = useQueueColumns('downloads')

const queueList = ref<{
  focusRow: (key: string) => Promise<boolean>
  revealRow: (key: string) => Promise<boolean>
} | null>(null)

const { draggingId, draggingFileId, onDrop, onFileDrop, onFileMove, onPackageMove } =
  useQueueReorder({ groups, filterActive, list: queueList, sorted: queueSort.active })
const {
  bulkBusy, packageControlBusy, copyLinks, copyPackageLinks, copyPath, deletePackage, bulkDeletePackages, changePackages,
  bulkAction, canControlPackage, controlPackage, bulkRemove, resetDownloads, bulkExtract, bulkRename, packageStorage,
  renamePackage, extractPackages, forceExtractPackage, renameFile, clearDownloads, clearEverything, removeDownload
} = useDownloadsActions({ selection, groups, packageDownloads })

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

/**
 * `f` puts the keyboard in the name search, as it does in the LinkGrabber's indexer search: one
 * key for "the search of this page", handed in while the list is mounted (RD-190-21).
 */
function focusSearch(): void {
  searchField.value?.querySelector('input')?.focus()
}

onMounted(() => {
  // `k` (RD-180-17): the same action as the menu item below, confirmation included.
  setClearCompletedAction(() => void clearDownloads('completed'))
  setIndexerSearchFocusAction(focusSearch)
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
  setClearCompletedAction(null)
  setIndexerSearchFocusAction(null)
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
  await Promise.all([fetchCategories(), fetchAccounts(), fetchProxies()])
}

function accountLabel(id: string | null | undefined): string | null {
  if (!id) return null
  const account = accounts.value.find(item => item.id === id)
  return account ? `${account.label} (${account.provider})` : null
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
</script>

<template>
  <UDashboardPanel id="downloads">
    <template #header>
      <UDashboardNavbar :title="t('downloads.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
        <template #right>
          <div data-tour="downloads-controls" class="flex items-center gap-2">
          <!-- On a phone the count goes (the toolbar repeats it) and the buttons keep only their
               icons, so the row fits beside the title and never covers the sidebar toggle. -->
          <UBadge color="neutral" variant="outline" class="font-mono max-sm:hidden">{{ t('common.units.file', { count: transfers.downloads.length }, transfers.downloads.length).toLocaleUpperCase() }}</UBadge>
          <QueuePauseControl placement="header" />
          <QueueResetFailedMenu :counts="resetFailed.counts.value" :busy="resetFailed.busy.value" @reset="resetFailed.resetShown" />
          <UDropdownMenu :items="clearItems">
            <UButton icon="i-lucide-list-x" :label="t('downloads.header.clear_list')" :aria-label="t('downloads.header.clear_list')" :title="t('downloads.header.clear_list')" :ui="{ label: 'max-sm:hidden' }" color="neutral" variant="outline" :loading="transfers.clearing" />
          </UDropdownMenu>
          </div>
        </template>
      </UDashboardNavbar>
      <!-- Wraps where the panel is narrow, as the LinkGrabber's toolbar does (RD-120-48). -->
      <UDashboardToolbar :ui="{ root: 'flex-wrap gap-y-1.5 py-1.5', left: 'min-w-0 flex-auto flex-wrap', right: 'ms-auto flex-wrap' }">
        <template #left>
          <div ref="searchField" class="w-full sm:w-56">
            <UInput
              v-model="search"
              type="search"
              icon="i-lucide-search"
              class="w-full"
              autocomplete="off"
              :placeholder="t('downloads.filters.search_placeholder')"
              :aria-label="t('downloads.filters.search_label')"
              data-testid="downloads-search"
            >
              <template #trailing><UKbd value="f" /></template>
            </UInput>
          </div>
          <USelect v-model="filter" :items="filters" value-key="value" class="w-36" :aria-label="t('downloads.filters.aria')" />
          <UCheckbox
            :model-value="selection.state.value === 'all' ? true : selection.state.value === 'some' ? 'indeterminate' : false"
            :disabled="!groups.length"
            :label="selection.count.value ? t('downloads.header.selection_count', { count: selection.count.value }) : t('common.actions.select_all')"
            :aria-label="t('downloads.header.select_all_hint')"
            :ui="{ root: 'shrink-0', label: 'whitespace-nowrap' }"
            @update:model-value="selection.toggleAll()"
          />
          <UButton :icon="openPackages.allOpen.value ? 'i-lucide-chevrons-down-up' : 'i-lucide-chevrons-up-down'" color="neutral" variant="ghost" :aria-label="t(openPackages.allOpen.value ? 'common.package_groups.close_all' : 'common.package_groups.open_all')" :title="t(openPackages.allOpen.value ? 'common.package_groups.close_all' : 'common.package_groups.open_all')" :disabled="!groups.length" data-testid="packages-open-toggle" @click="openPackages.toggleAll" />
        </template>
        <template #right>
          <UButton icon="i-lucide-file-down" color="neutral" variant="ghost" :aria-label="t('common.export.action_all')" :title="t('common.export.action_all')" :disabled="!transfers.packages.length" data-testid="downloads-export-all" @click="exportPackages({ all: true })" />
          <USwitch v-model="showMetadata" size="sm" :label="t('common.enrichment.show')" :title="t('common.enrichment.show_hint')" :ui="{ label: 'whitespace-nowrap' }" data-testid="show-metadata" />
          <span class="numeric whitespace-nowrap text-xs text-muted">{{ t('common.units.package', { count: groups.length }, groups.length) }} · {{ t('common.units.file', { count: visible.length }, visible.length) }}</span>
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

        <TorrentKillSwitchAlert />

        <CollisionPromptsAlert />

        <!-- Dismissible: a refusal stays until the next action or until it is closed, rather than being wiped by the next queue refresh. -->
        <UAlert
          v-if="transfers.error"
          color="error"
          icon="i-lucide-circle-alert"
          :description="transfers.error"
          close
          @update:open="transfers.error = null"
        />
        <UAlert v-if="transfers.notice" color="info" icon="i-lucide-info" :description="transfers.notice" />

        <PostprocessQueue v-if="postprocess.queue.length" :entries="postprocess.queue" />

        <section v-if="rows.length" class="space-y-2">
          <QueueSortNotice v-if="queueSort.sort.value" :sort="queueSort.sort.value" @reset="queueSort.reset" />
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
            <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-file-down" :label="t('common.export.action')" data-testid="downloads-export" @click="exportPackages({ downloadIds: selection.selectedIds.value })" />
            <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-refresh-cw" :label="t('downloads.reresolve.action')" :title="t('downloads.reresolve.hint')" data-testid="downloads-reresolve" @click="reresolve({ ids: selection.selectedIds.value })" />
            <UButton v-if="resettableSelection.length" size="sm" color="error" variant="outline" icon="i-lucide-rotate-ccw" :label="t('downloads.bulk.reset', { count: resettableSelection.length }, resettableSelection.length)" :loading="bulkBusy" @click="resetDownloads(selection.selectedIds.value)" />
            <UButton v-if="selection.fullySelectedPackageIds.value.length" size="sm" color="error" variant="outline" icon="i-lucide-package-x" :label="t('downloads.confirm.delete_packages_label', { count: selection.fullySelectedPackageIds.value.length }, selection.fullySelectedPackageIds.value.length)" :loading="bulkBusy" @click="bulkDeletePackages" />
          </BulkActionBar>
          <!--
            One flattened stream of rows through the shared list block: package headers and the
            files of the open ones, keyed so a row keeps its identity while the window slides.
            The capture-phase handlers read the shift key before the checkbox reports its new
            value, which is what turns a pick into a range (RD-106-12).
          -->
          <!-- While sorted for the eye the drag handles stay in the grid but are not shown (RD-1190-16). -->
          <div :style="columns.style.value" :class="queueSort.active.value ? '[&_[data-row-handle]]:invisible' : ''">
          <QueueColumnHeader
            :widths="columns.widths.value"
            view="downloads"
            :gutter="rows.length > DEFAULT_THRESHOLD"
            :customized="columns.customized.value"
            :sort="queueSort.sort.value"
            @resize="columns.setWidth"
            @reset="columns.reset"
            @reset-all="columns.resetAll"
            @sort="queueSort.toggle"
          />
          <VirtualRowList
            ref="queueList"
            :rows="rows"
            :label="t('downloads.list.aria', { count: rows.length })"
            @click.capture="selection.noteModifier"
            @keydown.capture="selection.noteModifier"
          >
            <template #row="{ row }">
              <PackageGroup
                v-if="row.kind === 'package'"
                :package="row.group.package"
                :hide-metadata="!showMetadata"
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
                :remote-targets="nzbHandOver.targets.value"
                :handed-over-to="nzbHandOver.packageHandedOverTo(row.group.package)"
                @select="selection.pickPackage"
                @toggle="openPackages.toggle"
                @open-all="openPackages.openAll"
                @close-all="openPackages.closeAll"
                @category="(id, categoryId) => changePackages([id], { categoryId })"
                @priority="(id, value) => changePackages([id], { priority: value })"
                @rename="renamePackage"
                @storage="packageStorage"
                @extract="(id) => extractPackages([id])"
                @force-extract="forceExtractPackage"
                @dragstart="(id) => draggingId = id"
                @drop="onDrop"
                @move="onPackageMove"
                @pause-package="(id) => controlPackage(id, 'pause')"
                @resume-package="(id) => controlPackage(id, 'resume')"
                @delete-package="deletePackage"
                @copy-path="copyPath"
                @copy-links="copyPackageLinks"
                @reset-failed="resetFailed.resetPackage"
                @export="(id: string) => exportPackages({ packageIds: [id] })"
                @reresolve="(id: string) => reresolve({ packageIds: [id] })"
                @hand-over="(_id: string, accountId: string) => void nzbHandOver.handOverPackage(row.group.package, accountId)"
              />
              <TransferCard
                v-else
                :class="fileFrame(row.group)"
                :download="row.download"
                :bytes-per-second="transfers.downloadRates[row.download.id] ?? 0"
                :eta-seconds="transfers.downloadEtas[row.download.id] ?? null"
                :waiting-for-host="transfers.downloadHostWaits[row.download.id] ?? null"
                :destination="row.group.package.destination"
                :account-label="accountLabel(row.download.account_id)"
                :selected="selection.selectedFiles.value.has(row.download.id)"
                @select="selection.pickFile"
                @pause="(id) => transfers.control(id, 'pause')"
                @resume="(id) => transfers.control(id, 'resume')"
                @cancel="(id) => transfers.control(id, 'cancel')"
                @stop-seeding="(id) => transfers.control(id, 'stop_seeding')"
                @remove="removeDownload"
                @reset="(id) => resetDownloads([id])"
                @rename="renameFile"
                @copy-path="copyPath"
                @copy-links="copyLinks"
                @reresolve="(id: string) => reresolve({ ids: [id] })"
                @dragstart="(id) => draggingFileId = id"
                @drop="onFileDrop"
                @move="onFileMove"
              />
            </template>
          </VirtualRowList>
          </div>
        </section>

        <!-- The queue has files, the filter or the search hides all of them: say so, and offer the way back. -->
        <UEmpty
          v-else-if="filterActive && transfers.downloads.length"
          as="section"
          class="min-h-48"
          icon="i-lucide-search-x"
          :title="t('downloads.filters.no_match_title')"
          :description="t('downloads.filters.no_match_hint')"
          :actions="[{ icon: 'i-lucide-filter-x', color: 'neutral', variant: 'outline', label: t('downloads.filters.reset'), onClick: resetFilter }]"
          data-testid="downloads-no-match"
        />

        <!--
          "Nothing here" is only true once the queue fetch has settled. Until then the store's
          own loading flag is what the reader sees, and a failed fetch stays visible as the
          error alert above rather than dissolving into an empty queue (RD-104-07).
        -->
        <DataState v-else :loading="transfers.loading" :empty="!transfers.error" :rows="4">
          <UEmpty
            as="section"
            class="signal-grid min-h-72"
            icon="i-lucide-inbox"
            :title="t('downloads.empty.title')"
            :description="t('downloads.empty.hint')"
          />
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
