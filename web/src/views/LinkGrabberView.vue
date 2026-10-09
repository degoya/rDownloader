<script setup lang="ts">
import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import BulkActionBar from '@/components/BulkActionBar.vue'
import CollectorCandidateRow from '@/components/CollectorCandidateRow.vue'
import CollectorHosterFilter from '@/components/CollectorHosterFilter.vue'
import CollectorListFilters from '@/components/CollectorListFilters.vue'
import CollectorPackageGroup from '@/components/CollectorPackageGroup.vue'
import DataState from '@/components/DataState.vue'
import IndexerReviewList from '@/components/IndexerReviewList.vue'
import IndexerSearchDrawer from '@/components/IndexerSearchDrawer.vue'
import LinkGrabberNavbar from '@/components/LinkGrabberNavbar.vue'
import NzbHistoryModal from '@/components/NzbHistoryModal.vue'
import NzbImportGroup from '@/components/NzbImportGroup.vue'
import QueueColumnHeader from '@/components/QueueColumnHeader.vue'
import QueueListBar from '@/components/QueueListBar.vue'
import SiteRulePickPanel from '@/components/SiteRulePickPanel.vue'
import VirtualRowList from '@/components/VirtualRowList.vue'
import { setLinkGrabberActions } from '@/composables/linkGrabberActions'
import { refreshQueuedSources } from '@/composables/useQueuedSources'
import { useCopyLinks } from '@/composables/useCopyLinks'
import { consumeFileImportRequest, fileImportRequested } from '@/composables/nzbImportRequest'
import { useFileImport } from '@/composables/useFileImport'
import { useGrabberActions } from '@/composables/useGrabberActions'
import { useGrabberFacets } from '@/composables/useGrabberFacets'
import { useGrabberRows } from '@/composables/useGrabberRows'
import { grabberKey, useGrabberSelection } from '@/composables/useGrabberSelection'
import type { CollectorEntry } from '@/composables/useGrabberSelection'
import { useIntakeModal } from '@/composables/useIntakeModal'
import { useNzbHandOver } from '@/composables/useNzbHandOver'
import { usePackageExport } from '@/composables/usePackageExport'
import { useGrabberEnqueue } from '@/composables/useGrabberEnqueue'
import { useGrabberReorder } from '@/composables/useGrabberReorder'
import { useOpenSections } from '@/composables/useOpenSections'
import { usePackageOpenState } from '@/composables/usePackageOpenState'
import { useQueueColumns } from '@/composables/useQueueColumns'
import { useShowMetadata } from '@/composables/useShowMetadata'
import { DEFAULT_THRESHOLD } from '@/composables/useVirtualRows'
import { useCategories } from '@/stores/categories'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { usePublishedSelection } from '@/stores/selection'
import { sharedText } from '@/utils/sharedLinks'
import type { CollectorSort } from '@/utils/collectorSort'

const collector = useCollectorStore()
const nzb = useNzbImportsStore()
const openIntake = useIntakeModal()
const toast = useToast()
const copyLinks = useCopyLinks()
const { exportPackages } = usePackageExport()
const { t } = useI18n()

const { categories, fetchCategories } = useCategories()
const { importing: importingFiles, importFiles } = useFileImport(categories)
const bulkBusy = ref(false)
const sort = ref<CollectorSort>('manual')
const descending = ref(false)
/** The facets, the state filter and the hidden hosters (RD-110-19, RD-130-21). */
const facets = useGrabberFacets()
const { stateFilter, facetBusy, hiddenHosters, filterActive } = facets

/** Which groups are showing their other mirrors; closed is the default, as any expansion is. */
const openMirrors = useOpenSections({ defaultOpen: false })
/** Why a reorder did not happen. Shown instead of the silent `return` it used to be. */
const notice = ref<string | null>(null)

/** Which packages are open, remembered per browser; "all" is what the filters show (RD-1170-01). */
const openPackages = usePackageOpenState('linkgrabber', { known: () => collector.packages.map(pkg => pkg.id), shown: () => groups.value.map(group => group.package.id) })
/** The "Show metadata" switch: the enricher chips under the link names, per browser (RD-150-19). */
const showMetadata = useShowMetadata('linkgrabber')

const { groups, visibleLinks, nzbGroups, entries, rows, orderedSelectionKeys } = useGrabberRows({
  sort, descending, stateFilter, hidden: hiddenHosters.hidden, openPackages, openMirrors
})
const selection = useGrabberSelection(entries, orderedSelectionKeys)
// How much is ticked, shown in the status bar while this view is open (RD-170-14).
usePublishedSelection(selection.size)
/** "4 links in 3 packages · 1 NZB import": the tooltip of both selection counts (RD-1230-02). */
const selectionDetail = computed(() => {
  const links = selection.collectorIds.value
  const packages = new Set(collector.candidates.filter(candidate => selection.collectorIdSet.value.has(candidate.id)).map(candidate => candidate.package_id)).size
  const nzbs = selection.nzbIds.value.length
  const parts = links.length ? [t('common.selection.detail', { items: t('common.units.link', { count: links.length }, links.length), packages: t('common.units.package', { count: packages }, packages) })] : []
  return [...parts, ...(nzbs ? [t('linkgrabber.selection_nzbs', { count: nzbs }, nzbs)] : [])].join(' · ')
})
/** The navbar button and `f` open the indexer search (RD-1230-02). */
const indexerSearch = ref<{ openSearch: () => Promise<void> } | null>(null)

/** The data columns' widths, set on the container of the header row and the rows (RD-191-11). */
const columns = useQueueColumns('linkgrabber')

const grabberList = ref<{
  focusRow: (key: string) => Promise<boolean>
  revealRow: (key: string) => Promise<boolean>
} | null>(null)

const {
  draggingEntry, draggingCandidate,
  dropOnPackage, dropOnNzb, dropOnCandidate, moveCandidate, moveEntry
} = useGrabberReorder({ groups, nzbGroups, sort, filterActive, notice, list: grabberList })

const {
  removeSelected, removeProgress, moveSelected, editPackageDialog, renameCandidate, removePackage, removeCandidate,
  dissolveMirror, clearAll, enqueueNzb, deleteNzb, setNzbCategory, setNzbPriority, setCategory,
  setPriority, applyToSelection, setSelectionPostprocessLevel
} = useGrabberActions({ nzbGroups, selection, bulkBusy })

/** Border frame of a link row: the package's frame carried down its children. */
function linkFrame(entry: CollectorEntry): string {
  const selected = entry.candidates.some(candidate => selection.collectorIdSet.value.has(candidate.id))
  return `border-x border-b ${selected ? 'border-primary' : 'border-muted'}`
}

/**
 * Jumps to the first selected row and puts the keyboard on it.
 *
 * It may sit in a collapsed package, or far outside the window; both are why this exists.
 */
async function revealSelection(): Promise<void> {
  const [candidateId] = selection.collectorIds.value
  if (candidateId) {
    const owner = collector.candidates.find(candidate => candidate.id === candidateId)?.package_id
    if (owner && !openPackages.isOpen(owner)) {
      openPackages.set(owner, true)
      await nextTick()
    }
    await grabberList.value?.focusRow(`link:${candidateId}`)
    return
  }
  const [nzbId] = selection.nzbIds.value
  if (nzbId) await grabberList.value?.focusRow(`nzb:${nzbId}`)
}
const route = useRoute()
const router = useRouter()
/**
 * `?package=<id>`, from a remote job's package link (RD-1120-02): read before the shared-link
 * handling clears the query, and focused once the package is listed.
 */
const wantedPackage = ref(typeof route.query.package === 'string' ? route.query.package : null)
watch([rows, grabberList], ([current, list]) => {
  const key = `package:${wantedPackage.value}`
  if (!wantedPackage.value || !list || !current.some(row => row.key === key)) return
  wantedPackage.value = null
  void list.focusRow(key)
}, { immediate: true, flush: 'post' })
const checking = computed(() => collector.candidates.some(c => c.state === 'checking'))
// A failed import holds no files, so it is not something that can be queued (RD-108-20).
const enqueueableNzbIds = computed(() => nzbGroups.value.filter(entry => !entry.item.duplicate && entry.item.state !== 'failed').map(entry => entry.id))
const {
  enqueueAll, enqueuePackage, enqueueSelected, enqueueCandidate, visibleCandidateIds
} = useGrabberEnqueue({ groups, nzbGroups, enqueueableNzbIds, selection, sort, filterActive, notice, bulkBusy })
/** NZB imports to a provider's account instead of the queue (RD-191-13); a failed one holds no files. */
const nzbHandOver = useNzbHandOver('linkgrabber')
const handOverIds = (): string[] => selection.nzbIds.value.filter(id => nzb.imports.find(item => item.id === id)?.state === 'imported')

/**
 * The online check takes what the list shows, like the enqueue does (RD-130-21): a filter or a
 * hidden hoster narrows it, and a shown link's mirrors are checked with it, because they are
 * what the queue falls back to.
 */
function checkVisible(): void {
  void collector.checkLinks(visibleCandidateIds(groups.value.map(group => group.package.id)))
}

// Collector refresh + SSE live app-wide in App.vue so the nav badge stays current, so entering
// this route must not refetch: three more GETs per visit only crowded the connection pool.
/**
 * `a`, `e`, `w` and `r` (1.8.1): the buttons in the navbar, handed to the shortcut catalogue
 * while this view is mounted. A key does nothing where its button is disabled.
 */
const canEnqueue = (): boolean => entries.value.length > 0 && !checking.value && !collector.pending
onUnmounted(() => setLinkGrabberActions(null))

onMounted(() => {
  setLinkGrabberActions({
    addLinks: () => void addLinks(),
    enqueueAll: () => { if (canEnqueue()) void enqueueAll(false) },
    enqueuePaused: () => { if (canEnqueue()) void enqueueAll(true) },
    clearAll: () => { if (entries.value.length) void clearAll() }
  })
  void collector.loadMirrorPreference()
  void handleSharedLinks()
  // A pending import request (cross-route drop handoff, `n` shortcut) must wait for categories
  // to load first, otherwise openNzbImport() snapshots an empty list into the open modal.
  void fetchCategories().finally(handlePendingImportRequest)
})

// The queue's copies of these links (RD-150-01), asked again whenever the address list changes.
watch(
  () => collector.candidates.map(candidate => candidate.url).join('\n'),
  (joined) => { void refreshQueuedSources(joined ? joined.split('\n') : []) },
  { immediate: true }
)

// A window-level NZB drop (useNzbDropZone) navigates here and stashes files in the handoff
// module; a request already pending on mount is picked up above, one arriving while this view
// stays mounted (e.g. dropping again without leaving /linkgrabber) is picked up here.
watch(fileImportRequested, (pending) => {
  if (pending) handlePendingImportRequest()
})

// The hoster facet is no longer reset when the last link of that hoster goes (RD-110-19): it
// is a standing preference stored on the server, and clearing it because this list happens to
// be empty right now would throw away a decision the next package still needs. The select
// keeps offering the value in force, so it can always be cleared by hand.

/** Every link of the package, whatever the filters hide: the package is what was asked for (RD-190-21). */
function copyPackageLinks(id: string): void {
  void copyLinks(collector.candidates.filter(candidate => candidate.package_id === id).map(candidate => candidate.url))
}

/**
 * Takes over a link shared from a mobile browser (RD-090-08).
 *
 * The share target is a GET target, so the link arrives in the query string. It is consumed
 * once and removed from the URL: a reload must not hand the same link over a second time,
 * and the address bar should not keep showing what was shared.
 */
async function handleSharedLinks(): Promise<void> {
  const text = sharedText(route.query as Record<string, unknown>)
  await router.replace({ path: route.path })
  if (!text) return
  await collector.collect({ text })
}

function handlePendingImportRequest(): void {
  const request = consumeFileImportRequest()
  if (request) void importFiles(request.files)
}

async function addLinks(): Promise<void> {
  const result = await openIntake()
  if (!result) return
  const outcome = await collector.collect({ text: result.text, ...(result.packageName ? { packageName: result.packageName } : {}), ...(result.password ? { password: result.password } : {}) })
  if (outcome.skippedExcluded) {
    toast.add({ title: t('linkgrabber.intake.skipped_excluded', { count: outcome.skippedExcluded }, outcome.skippedExcluded), color: 'warning', icon: 'i-lucide-shield-ban' })
  }
  if (outcome.skippedDisabled) {
    toast.add({ title: t('linkgrabber.intake.skipped_disabled', { count: outcome.skippedDisabled }, outcome.skippedDisabled), color: 'warning', icon: 'i-lucide-power-off' })
  }
  // Both numbers, not just the loss: "12 of 40" says a rule reached too far, while a bare
  // "12 dropped" reads like a fault in the paste (RD-110-07).
  if (outcome.listed) {
    toast.add({ title: t('linkgrabber.intake.listed', { count: outcome.listed }, outcome.listed), color: 'info', icon: 'i-lucide-list-checks' })
  }
  if (outcome.crawledDropped) {
    toast.add({ title: t('linkgrabber.intake.crawled_dropped', { count: outcome.crawledDropped, found: outcome.crawledFound }, outcome.crawledDropped), color: 'warning', icon: 'i-lucide-file-x' })
  }
}

/**
 * The server de-duplicates NZBs by SHA-256 and returns the existing row instead of a new one.
 * An already enqueued duplicate never reaches the LinkGrabber list, so every outcome is reported
 * explicitly — otherwise the import looks like it silently did nothing.
 */
const overlay = useOverlay()
const nzbHistoryModal = overlay.create(NzbHistoryModal)

/** Shows every stored NZB import (also enqueued ones the LinkGrabber list hides). */
function openNzbHistory(): void {
  void nzb.refresh()
  nzbHistoryModal.open()
}
</script>

<template>
  <UDashboardPanel id="linkgrabber">
    <template #header>
      <LinkGrabberNavbar
        :importing="importingFiles || nzb.pending"
        :checking="checking"
        :can-check="!!collector.candidates.length && !(filterActive && !visibleLinks)"
        :has-entries="!!entries.length"
        :enqueuing="collector.pending"
        @add="addLinks"
        @import="importFiles()"
        @history="openNzbHistory"
        @search="indexerSearch?.openSearch()"
        @check="checkVisible"
        @enqueue="(paused: boolean) => enqueueAll(paused)"
        @clear-all="clearAll"
      />
    </template>

    <template #body>
      <div data-tour="grabber-body" class="flex w-full flex-col gap-4">
        <!-- Behind the navbar button and `f` (RD-1230-02); without an indexer it says where to add one. -->
        <IndexerSearchDrawer ref="indexerSearch" />
        <UAlert v-if="collector.error" color="error" :description="collector.error" />
        <UAlert v-if="nzb.error" color="error" :description="nzb.error" />
        <UAlert v-if="notice" color="info" icon="i-lucide-info" :description="notice" />
        <SiteRulePickPanel />
        <CollectorHosterFilter
          :hosters="hiddenHosters.hosters.value"
          :hidden-links="hiddenHosters.hiddenLinks.value.length"
          :hidden-hosters="hiddenHosters.hiddenHosterCount.value"
          :busy="hiddenHosters.busy.value || facetBusy"
          @toggle="(hoster: string, hide: boolean) => void hiddenHosters.setHidden(hoster, hide)"
          @show-all="hiddenHosters.showAll()"
        />
        <section class="space-y-2">
          <!-- The row at the list (RD-1230-02), in the order of the Downloads one (`QueueListBar`). -->
          <QueueListBar
            v-model:show-metadata="showMetadata"
            :state="selection.state.value"
            :count="selection.count.value"
            :detail="selectionDetail"
            :select-hint="t('linkgrabber.select_all_hint')"
            :empty="!entries.length"
            :all-open="openPackages.allOpen.value"
            :export-disabled="!collector.packages.length"
            :count-text="`${t('common.units.package', { count: entries.length }, entries.length)} · ${filterActive ? t('linkgrabber.filter.count', { visible: visibleLinks, total: collector.candidates.length }) : t('common.units.link', { count: collector.candidates.length }, collector.candidates.length)}`"
            @toggle-all="selection.toggleAll()"
            @toggle-open="openPackages.toggleAll"
            @export-all="exportPackages({ collectorPackageIds: collector.packages.map(pkg => pkg.id) })"
          >
            <template #filters>
              <CollectorListFilters v-model:sort="sort" v-model:descending="descending" :facets="facets" @regroup="collector.regroup()" />
            </template>
            <template #end>
              <UButton icon="i-lucide-refresh-cw" color="neutral" variant="ghost" :aria-label="t('common.actions.refresh')" :title="t('common.actions.refresh')" @click="collector.refresh" />
            </template>
          </QueueListBar>
          <BulkActionBar
            v-if="selection.count.value"
            :count="selection.count.value"
            :detail="selectionDetail"
            :categories="categories"
            :busy="bulkBusy"
            :package-actions-disabled="!selection.count.value"
            :level-disabled="!selection.collectorIds.value.length"
            :export-disabled="!selection.collectorIds.value.length"
            @category="(categoryId) => applyToSelection({ categoryId })"
            @priority="(priority) => applyToSelection({ priority })"
            @postprocess="(level) => setSelectionPostprocessLevel(level)"
            @enqueue="enqueueSelected()"
            @enqueue-paused="enqueueSelected(true)"
            @reveal="revealSelection"
            @export="exportPackages({ collectorPackageIds: selection.collectorIds.value })"
            @remove="removeSelected"
            @clear="selection.clear()"
          >
            <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-folder-input" :aria-label="t('linkgrabber.actions.move_to_new_package')" :title="t('linkgrabber.actions.move_to_new_package')" :disabled="!selection.collectorIds.value.length" :loading="bulkBusy" @click="moveSelected" />
            <UDropdownMenu v-if="selection.nzbIds.value.length && nzbHandOver.targets.value.length" :items="nzbHandOver.menuItems(handOverIds)">
              <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-cloud-upload" :aria-label="t('linkgrabber.nzb.hand_over.action')" :title="t('linkgrabber.nzb.hand_over.hint')" :disabled="!handOverIds().length" data-testid="grabber-hand-over" />
            </UDropdownMenu>
            <span v-if="removeProgress" class="numeric text-xs text-muted" role="status" data-testid="grabber-remove-progress">
              {{ t('linkgrabber.bulk.removing', { done: removeProgress.done, total: removeProgress.total }) }}
            </span>
          </BulkActionBar>
          <!--
            One flattened stream of rows through the shared list block: package headers, the links
            of the open ones, and the reviewed NZB imports between them. The capture-phase handlers
            read the shift key before a checkbox reports its new value (RD-106-12).
          -->
          <div v-if="rows.length" :style="columns.style.value">
          <QueueColumnHeader
            :widths="columns.widths.value"
            view="linkgrabber"
            :gutter="rows.length > DEFAULT_THRESHOLD"
            :customized="columns.customized.value"
            @resize="columns.setWidth"
            @reset="columns.reset"
            @reset-all="columns.resetAll"
          />
          <VirtualRowList
            ref="grabberList"
            :rows="rows"
            :label="t('linkgrabber.list.aria', { count: rows.length })"
            @click.capture="selection.noteModifier"
            @keydown.capture="selection.noteModifier"
          >
            <template #row="{ row }">
              <CollectorPackageGroup
                v-if="row.kind === 'package'"
                :package="row.entry.package"
                :candidates="row.entry.candidates"
                :categories="categories"
                :selected-ids="selection.collectorIdSet.value"
                :enqueuing-ids="collector.enqueuingIds"
                :dragging="draggingEntry === grabberKey('collector', row.entry.id)"
                :open="openPackages.isOpen(row.entry.id)"
                @select="(_ids: string[], value: boolean) => selection.pickPackage(row.entry.id, value)"
                @toggle="openPackages.toggle"
                @open-all="openPackages.openAll"
                @close-all="openPackages.closeAll"
                @category="(id, categoryId) => setCategory([id], categoryId)"
                @priority="(id, value) => setPriority([id], value)"
                @rename="editPackageDialog"
                @enqueue="enqueuePackage"
                @enqueue-paused="(id: string) => enqueuePackage(id, true)"
                @remove="removePackage"
                @copy-links="copyPackageLinks"
                @export="(id: string) => exportPackages({ collectorPackageIds: [id] })"
                @dragstart="(id: string) => draggingEntry = grabberKey('collector', id)"
                @drop="dropOnPackage"
                @move="(id: string, delta: -1 | 1) => moveEntry('collector', id, delta)"
              />
              <CollectorCandidateRow
                v-else-if="row.kind === 'candidate'"
                :class="linkFrame(row.entry)"
                :candidate="row.candidate"
                :hide-metadata="!showMetadata"
                :selected="selection.collectorIdSet.value.has(row.candidate.id)"
                :busy="collector.enqueuingIds.has(row.candidate.id)"
                :mirror-group="row.group ?? null"
                :mirror-open="row.group ? openMirrors.isOpen(row.group.key) : false"
                :mirror-member="row.member === true"
                @toggle-mirror="openMirrors.toggle"
                @choose-mirror="(id: string, chosen: boolean) => void collector.chooseMirror(id, chosen)"
                @dissolve-mirror="dissolveMirror"
                @hide-hoster="(hoster: string) => void hiddenHosters.setHidden(hoster, true)"
                @select="selection.pickCollector"
                @rename="renameCandidate"
                @enqueue="enqueueCandidate"
                @remove="removeCandidate"
                @copy-links="copyLinks"
                @dragstart="(id) => draggingCandidate = id"
                @drop="dropOnCandidate"
                @variant="(id, variantId) => void collector.setMediaVariant(id, variantId)"
                @move="moveCandidate"
              />
              <NzbImportGroup
                v-else
                :item="row.entry.item"
                :categories="categories"
                :selected="selection.isNzbSelected(row.entry.id)"
                :enqueuing="nzb.enqueuingIds.has(row.entry.id)"
                :deleting="nzb.deletingIds.has(row.entry.id)"
                :dragging="draggingEntry === grabberKey('nzb', row.entry.id)"
                :remote-targets="nzbHandOver.targets.value"
                :handed-over-to="nzbHandOver.handedOverTo(row.entry.item)"
                :handing-over="nzb.handingOverIds.has(row.entry.id)"
                @select="selection.pickNzb"
                @category="setNzbCategory"
                @priority="setNzbPriority"
                @enqueue="enqueueNzb"
                @enqueue-paused="(id: string) => enqueueNzb(id, true)"
                @remove="deleteNzb"
                @hand-over="(id: string, accountId: string) => void nzbHandOver.handOver([id], accountId)"
                @dragstart="(id: string) => draggingEntry = grabberKey('nzb', id)"
                @drop="dropOnNzb"
                @move="(id: string, delta: -1 | 1) => moveEntry('nzb', id, delta)"
              />
            </template>
          </VirtualRowList>
          </div>
          <!-- The collector's fetch, not just its result: "no links" waits for it (RD-104-07). -->
          <DataState v-else :loading="collector.loading" :empty="!collector.error" :rows="3">
            <UEmpty
              class="signal-grid min-h-60"
              icon="i-lucide-magnet"
              :title="t('linkgrabber.empty_title')"
              :description="t('linkgrabber.empty')"
            />
          </DataState>
        </section>
      </div>
        <IndexerReviewList />
    </template>
  </UDashboardPanel>
</template>
