<script setup lang="ts">
import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import { api } from '@/api/client'
import type { Category } from '@/api/types'
import BulkActionBar from '@/components/BulkActionBar.vue'
import CollectorCandidateRow from '@/components/CollectorCandidateRow.vue'
import CollectorHosterFilter from '@/components/CollectorHosterFilter.vue'
import CollectorPackageGroup from '@/components/CollectorPackageGroup.vue'
import DataState from '@/components/DataState.vue'
import IndexerReviewList from '@/components/IndexerReviewList.vue'
import IndexerSearchPanel from '@/components/IndexerSearchPanel.vue'
import NzbHistoryModal from '@/components/NzbHistoryModal.vue'
import NzbImportGroup from '@/components/NzbImportGroup.vue'
import VirtualRowList from '@/components/VirtualRowList.vue'
import { refreshQueuedSources } from '@/composables/useQueuedSources'
import { consumeFileImportRequest, fileImportRequested } from '@/composables/nzbImportRequest'
import { useFileImport } from '@/composables/useFileImport'
import { useGrabberActions } from '@/composables/useGrabberActions'
import { useGrabberFacets } from '@/composables/useGrabberFacets'
import { useGrabberRows } from '@/composables/useGrabberRows'
import { grabberKey, useGrabberSelection } from '@/composables/useGrabberSelection'
import type { CollectorEntry } from '@/composables/useGrabberSelection'
import { useIntakeModal } from '@/composables/useIntakeModal'
import { useGrabberEnqueue } from '@/composables/useGrabberEnqueue'
import { useGrabberReorder } from '@/composables/useGrabberReorder'
import { useOpenSections } from '@/composables/useOpenSections'
import { useShowMetadata } from '@/composables/useShowMetadata'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { usePublishedSelection } from '@/stores/selection'
import { sharedText } from '@/utils/sharedLinks'
import { SORT_OPTIONS, type CollectorSort } from '@/utils/collectorSort'

const collector = useCollectorStore()
const nzb = useNzbImportsStore()
const openIntake = useIntakeModal()
const toast = useToast()
const { t } = useI18n()

const categories = ref<Category[]>([])
const { importing: importingFiles, importFiles } = useFileImport(categories)
const bulkBusy = ref(false)
const sort = ref<CollectorSort>('manual')
const descending = ref(false)
/** The facets, the state filter and the hidden hosters (RD-110-19, RD-130-21). */
const {
  hosterFilter, qualityFilter, languageFilter, stateFilter, facetBusy, hiddenHosters,
  facetFilterActive, filterActive, hosterItems, qualityItems, languageItems, stateItems, clearFilters
} = useGrabberFacets()

/** Which groups are showing their other mirrors; closed is the default, as any expansion is. */
const openMirrors = useOpenSections({ defaultOpen: false })
/** Why a reorder did not happen. Shown instead of the silent `return` it used to be. */
const notice = ref<string | null>(null)

const sortItems = computed(() => SORT_OPTIONS.map(option => ({ label: t(option.labelKey), value: option.value })))

const openPackages = useOpenSections({ defaultOpen: true })
/** The "Show metadata" switch: the enricher chips under the link names, per browser (RD-150-19). */
const showMetadata = useShowMetadata('linkgrabber')

const { groups, visibleLinks, nzbGroups, entries, rows, orderedSelectionKeys } = useGrabberRows({
  sort, descending, stateFilter, hidden: hiddenHosters.hidden, openPackages, openMirrors
})
const selection = useGrabberSelection(entries, orderedSelectionKeys)
// How much is ticked, shown in the status bar while this view is open (RD-170-14).
usePublishedSelection(selection.size)

const grabberList = ref<{
  focusRow: (key: string) => Promise<boolean>
  revealRow: (key: string) => Promise<boolean>
} | null>(null)

const {
  draggingEntry, draggingCandidate,
  dropOnPackage, dropOnNzb, dropOnCandidate, moveCandidate, moveEntry
} = useGrabberReorder({ groups, nzbGroups, sort, filterActive, notice, list: grabberList })

const {
  removeSelected, moveSelected, editPackageDialog, renameCandidate, removePackage, removeCandidate,
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
const checking = computed(() => collector.candidates.some(c => c.state === 'checking'))
// A failed import holds no files, so it is not something that can be queued (RD-108-20).
const enqueueableNzbIds = computed(() => nzbGroups.value.filter(entry => !entry.item.duplicate && entry.item.state !== 'failed').map(entry => entry.id))
const {
  enqueueAll, enqueuePackage, enqueueSelected, enqueueCandidate, visibleCandidateIds
} = useGrabberEnqueue({ groups, nzbGroups, enqueueableNzbIds, selection, sort, filterActive, notice, bulkBusy })

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
onMounted(() => {
  void collector.loadMirrorPreference()
  void handleSharedLinks()
  // A pending import request (cross-route drop handoff, `n` shortcut) must wait for categories
  // to load first, otherwise openNzbImport() snapshots an empty list into the open modal.
  void loadCategories().finally(handlePendingImportRequest)
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

async function loadCategories(): Promise<void> {
  const response = await api.GET('/api/v1/categories')
  if (response.data) categories.value = response.data
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
      <UDashboardNavbar :title="t('linkgrabber.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
        <template #right>
          <div data-tour="grabber-add" class="flex items-center gap-2">
          <UButton icon="i-lucide-plus" :label="t('linkgrabber.actions.add_links')" color="neutral" variant="outline" @click="addLinks" />
          <UButton icon="i-lucide-file-up" :label="t('linkgrabber.actions.import_files')" color="neutral" variant="outline" :loading="importingFiles || nzb.pending" @click="() => importFiles()" />
          <UButton icon="i-lucide-history" color="neutral" variant="outline" :aria-label="t('linkgrabber.nzb.history.title')" :title="t('linkgrabber.nzb.history.title')" @click="openNzbHistory" />
          <UButton icon="i-lucide-radar" :label="t('linkgrabber.actions.check_links')" color="neutral" variant="outline" :loading="checking" :disabled="!collector.candidates.length || (filterActive && !visibleLinks)" @click="checkVisible" />
          <!-- The one solid button in this bar. Getting the reviewed links into the queue is what
               the LinkGrabber is for; adding and importing are how they arrive, and they read as
               the neutral pair they belong to. As `soft` beside a solid "Add links" this sat
               below the action that only fills the list it is meant to empty. -->
          <UButton icon="i-lucide-list-end" :label="t('linkgrabber.actions.enqueue_all')" :disabled="!entries.length || checking" :loading="collector.pending" @click="enqueueAll(false)" />
          <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" color="neutral" variant="outline" :title="t('linkgrabber.actions.enqueue_paused_hint')" :disabled="!entries.length || checking" :loading="collector.pending" @click="enqueueAll(true)" />
          <UButton icon="i-lucide-list-x" :label="t('linkgrabber.actions.clear_all')" color="error" variant="soft" :disabled="!entries.length" @click="clearAll" />
          </div>
        </template>
      </UDashboardNavbar>
      <!-- The facets wrap onto a second row where the panel is too narrow for them, rather than
           squeezing "Select all" onto two lines or scrolling the count out of view (RD-120-48). -->
      <UDashboardToolbar :ui="{ root: 'py-1.5', left: 'min-w-0 flex-1 flex-wrap' }">
        <template #left>
          <UCheckbox
            :model-value="selection.state.value === 'all' ? true : selection.state.value === 'some' ? 'indeterminate' : false"
            :disabled="!entries.length"
            :label="selection.count.value ? t('linkgrabber.selection_count', { count: selection.count.value }) : t('common.actions.select_all')"
            :aria-label="t('linkgrabber.select_all_hint')"
            :ui="{ root: 'shrink-0', label: 'whitespace-nowrap' }"
            @update:model-value="selection.toggleAll()"
          />
          <USelect v-model="sort" :items="sortItems" value-key="value" class="w-40" :aria-label="t('linkgrabber.sort.label')" />
          <UButton :icon="descending ? 'i-lucide-arrow-down-wide-narrow' : 'i-lucide-arrow-up-narrow-wide'" color="neutral" variant="ghost" :aria-label="descending ? t('linkgrabber.sort.descending') : t('linkgrabber.sort.ascending')" :disabled="sort === 'manual'" @click="descending = !descending" />
          <!-- The three facets. Inside a mirror group they choose the member the queue will
               fetch; outside one they hide what cannot satisfy them, and they stay set for the
               next package (RD-110-19, `design.md`). -->
          <USelect v-model="qualityFilter" :items="qualityItems" value-key="value" class="w-36" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.quality_label')" :title="t('linkgrabber.filter.facet_hint')" />
          <USelect v-model="languageFilter" :items="languageItems" value-key="value" class="w-36" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.language_label')" :title="t('linkgrabber.filter.facet_hint')" />
          <USelect v-model="hosterFilter" :items="hosterItems" value-key="value" class="w-44" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.hoster_label')" :title="t('linkgrabber.filter.facet_hint')" />
          <USelect v-model="stateFilter" :items="stateItems" value-key="value" class="w-36" :aria-label="t('linkgrabber.filter.state_label')" />
          <UButton v-if="facetFilterActive" icon="i-lucide-filter-x" color="neutral" variant="ghost" size="sm" :disabled="facetBusy" :aria-label="t('linkgrabber.filter.clear')" :title="t('linkgrabber.filter.clear')" @click="clearFilters" />
          <UButton icon="i-lucide-group" :label="t('linkgrabber.actions.regroup')" color="neutral" variant="ghost" size="sm" @click="collector.regroup()" />
        </template>
        <template #right>
          <USwitch v-model="showMetadata" size="sm" :label="t('common.enrichment.show')" :title="t('common.enrichment.show_hint')" :ui="{ label: 'whitespace-nowrap' }" data-testid="show-metadata" />
          <span class="numeric whitespace-nowrap text-xs text-muted">{{ t('common.units.package', { count: entries.length }, entries.length) }} · {{ filterActive ? t('linkgrabber.filter.count', { visible: visibleLinks, total: collector.candidates.length }) : t('common.units.link', { count: collector.candidates.length }, collector.candidates.length) }}</span>
          <UButton icon="i-lucide-refresh-cw" color="neutral" variant="ghost" :aria-label="t('common.actions.refresh')" @click="collector.refresh" />
        </template>
      </UDashboardToolbar>
    </template>

    <template #body>
      <div data-tour="grabber-body" class="flex w-full flex-col gap-4">
        <!-- Always shown, disabled with a hint until an indexer is enabled; `f` focuses it (RD-180-19). -->
        <IndexerSearchPanel />
        <UAlert v-if="collector.error" color="error" variant="subtle" :description="collector.error" />
        <UAlert v-if="nzb.error" color="error" variant="subtle" :description="nzb.error" />
        <UAlert v-if="notice" color="info" variant="subtle" icon="i-lucide-info" :description="notice" />
        <CollectorHosterFilter
          :hosters="hiddenHosters.hosters.value"
          :hidden-links="hiddenHosters.hiddenLinks.value.length"
          :hidden-hosters="hiddenHosters.hiddenHosterCount.value"
          :busy="hiddenHosters.busy.value || facetBusy"
          @toggle="(hoster: string, hide: boolean) => void hiddenHosters.setHidden(hoster, hide)"
          @show-all="hiddenHosters.showAll()"
        />
        <BulkActionBar
          v-if="selection.count.value"
          :count="selection.count.value"
          :categories="categories"
          :busy="bulkBusy"
          :package-actions-disabled="!selection.count.value"
          :level-disabled="!selection.collectorIds.value.length"
          @category="(categoryId) => applyToSelection({ categoryId })"
          @priority="(priority) => applyToSelection({ priority })"
          @postprocess="(level) => setSelectionPostprocessLevel(level)"
          @enqueue="enqueueSelected()"
          @enqueue-paused="enqueueSelected(true)"
          @remove="removeSelected"
          @clear="selection.clear()"
        >
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-crosshair" :label="t('common.actions.reveal')" @click="revealSelection" />
          <UButton size="sm" color="neutral" variant="outline" icon="i-lucide-folder-input" :label="t('linkgrabber.actions.move_to_new_package')" :disabled="!selection.collectorIds.value.length" :loading="bulkBusy" @click="moveSelected" />
        </BulkActionBar>
        <!--
          One flattened stream of rows through the shared list block: package headers, the links
          of the open ones, and the reviewed NZB imports between them. The capture-phase handlers
          read the shift key before a checkbox reports its new value (RD-106-12).
        -->
        <VirtualRowList
          v-if="rows.length"
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
              @category="(id, categoryId) => setCategory([id], categoryId)"
              @priority="(id, value) => setPriority([id], value)"
              @rename="editPackageDialog"
              @enqueue="enqueuePackage"
              @enqueue-paused="(id: string) => enqueuePackage(id, true)"
              @remove="removePackage"
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
              @select="selection.pickNzb"
              @category="setNzbCategory"
              @priority="setNzbPriority"
              @enqueue="enqueueNzb"
              @enqueue-paused="(id: string) => enqueueNzb(id, true)"
              @remove="deleteNzb"
              @dragstart="(id: string) => draggingEntry = grabberKey('nzb', id)"
              @drop="dropOnNzb"
              @move="(id: string, delta: -1 | 1) => moveEntry('nzb', id, delta)"
            />
          </template>
        </VirtualRowList>
        <!-- The collector's fetch, not just its result: "no links" waits for it (RD-104-07). -->
        <DataState v-else :loading="collector.loading" :empty="!collector.error" :rows="3">
          <div class="signal-grid grid min-h-60 place-items-center border border-dashed border-muted p-8 text-center text-sm text-muted">
            {{ t('linkgrabber.empty') }}
          </div>
        </DataState>
      </div>
        <IndexerReviewList />
    </template>
  </UDashboardPanel>
</template>
