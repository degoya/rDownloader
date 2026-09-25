<script setup lang="ts">
import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import { api } from '@/api/client'
import type { Category, DownloadPriority, LinkCandidate, PostprocessLevel } from '@/api/types'
import BulkActionBar from '@/components/BulkActionBar.vue'
import CollectorCandidateRow from '@/components/CollectorCandidateRow.vue'
import CollectorHosterFilter from '@/components/CollectorHosterFilter.vue'
import CollectorPackageGroup from '@/components/CollectorPackageGroup.vue'
import DataState from '@/components/DataState.vue'
import IndexerReviewList from '@/components/IndexerReviewList.vue'
import NzbHistoryModal from '@/components/NzbHistoryModal.vue'
import NzbImportGroup from '@/components/NzbImportGroup.vue'
import VirtualRowList from '@/components/VirtualRowList.vue'
import { consumeFileImportRequest, fileImportRequested } from '@/composables/nzbImportRequest'
import { useFileImport } from '@/composables/useFileImport'
import { useConfirm } from '@/composables/useConfirm'
import { grabberKey, isSelectableCandidate, mergeGrabberEntries, useGrabberSelection, type CollectorEntry, type NzbEntry } from '@/composables/useGrabberSelection'
import { useIntakeModal } from '@/composables/useIntakeModal'
import { useGrabberEnqueue } from '@/composables/useGrabberEnqueue'
import { useGrabberReorder } from '@/composables/useGrabberReorder'
import { useHiddenHosters } from '@/composables/useHiddenHosters'
import { useOpenSections } from '@/composables/useOpenSections'
import type { VirtualRow } from '@/composables/useVirtualRows'
import { packageEditChange, usePackageEdit } from '@/composables/usePackageEdit'
import { useRename } from '@/composables/useRename'
import { useCollectorStore, type EnqueueBatchResult } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { useTransfersStore } from '@/stores/transfers'
import { sharedText } from '@/utils/sharedLinks'
import { SORT_OPTIONS, hosterOf, sortCandidates, sortCollectorEntries, type CollectorSort } from '@/utils/collectorSort'
import { facetValues, filterRows, hideHosters, mirrorRows, type MirrorFacet, type MirrorGroup } from '@/utils/mirrorGroups'
import { withBase } from '@/basePath'

const collector = useCollectorStore()
const nzb = useNzbImportsStore()
const transfers = useTransfersStore()
const confirm = useConfirm()
const openIntake = useIntakeModal()
const editPackage = usePackageEdit()
const rename = useRename()
const toast = useToast()
const { t } = useI18n()

const categories = ref<Category[]>([])
const { importing: importingFiles, importFiles } = useFileImport(categories)
const bulkBusy = ref(false)
const sort = ref<CollectorSort>('manual')
const descending = ref(false)
/**
 * `'all'` = facet off; an empty string is not a legal select value (Reka UI throws on it).
 *
 * The three facets are one control with two effects (RD-110-19, `design.md`): inside a mirror
 * group the server uses them to choose the member the queue will fetch, and outside one they
 * hide what cannot satisfy them. They are the standing preference, so they are written to the
 * server rather than kept here — this ref only mirrors the last answer it gave.
 */
const hosterFilter = computed({
  get: () => collector.mirrorPreference.hoster ?? 'all',
  set: (value: string) => void applyFacet('hoster', value)
})
const qualityFilter = computed({
  get: () => collector.mirrorPreference.quality ?? 'all',
  set: (value: string) => void applyFacet('quality', value)
})
const languageFilter = computed({
  get: () => collector.mirrorPreference.language ?? 'all',
  set: (value: string) => void applyFacet('language', value)
})
/** True while a facet change is in flight; the selects stay readable but refuse a second one. */
const facetBusy = ref(false)

async function applyFacet(facet: MirrorFacet, value: string): Promise<void> {
  facetBusy.value = true
  await collector.setMirrorPreference({
    ...collector.mirrorPreference,
    [facet]: value === 'all' ? null : value
  })
  facetBusy.value = false
}

/** Which groups are showing their other mirrors; closed is the default, as any expansion is. */
const openMirrors = useOpenSections({ defaultOpen: false })
const stateFilter = ref<LinkCandidate['state'] | 'all'>('all')
/** Why a reorder did not happen. Shown instead of the silent `return` it used to be. */
const notice = ref<string | null>(null)

/** Hosters hidden from the list, several at once (RD-130-21); stored with the facets. */
const hiddenHosters = useHiddenHosters()

const sortItems = computed(() => SORT_OPTIONS.map(option => ({ label: t(option.labelKey), value: option.value })))
/** The facets and the state filter, which "clear filters" resets; hidden hosters have their own way back. */
const facetFilterActive = computed(() => hosterFilter.value !== 'all' || stateFilter.value !== 'all'
  || qualityFilter.value !== 'all' || languageFilter.value !== 'all')
/**
 * Whether the list shows less than the LinkGrabber holds. Hidden hosters count only while they
 * hide a link: a hoster hidden last week with nothing in the list now must not turn every
 * enqueue into a partial one or refuse a reorder of a list that is in fact whole.
 */
const filterActive = computed(() => facetFilterActive.value || hiddenHosters.hiddenLinks.value.length > 0)
/** Hoster options come from the unfiltered list so the active choice never disappears. */
const hosterItems = computed(() => {
  const values = new Set([...collector.candidates.map(hosterOf)].filter(Boolean))
  if (collector.mirrorPreference.hoster) values.add(collector.mirrorPreference.hoster)
  return [
    { label: t('linkgrabber.filter.all_hosters'), value: 'all' },
    ...[...values].sort().map(hoster => ({ label: hoster, value: hoster }))
  ]
})
/**
 * The values each facet actually takes in this list, so the select offers no dead option. The
 * value in force is added back even when nothing carries it any more: a preference that
 * vanished from its own control could not be cleared.
 */
function facetItems(facet: 'quality' | 'language', allLabel: string) {
  const values = facetValues(collector.candidates, facet)
  const current = collector.mirrorPreference[facet]
  if (current && !values.includes(current)) values.push(current)
  return [{ label: allLabel, value: 'all' }, ...values.sort().map(value => ({ label: value, value }))]
}
const qualityItems = computed(() => facetItems('quality', t('linkgrabber.filter.all_qualities')))
const languageItems = computed(() => facetItems('language', t('linkgrabber.filter.all_languages')))
const stateItems = computed(() => [
  { label: t('linkgrabber.filter.all_states'), value: 'all' },
  ...(['online', 'offline', 'duplicate', 'checking', 'resolving', 'unsupported', 'error'] as const)
    .map(state => ({ label: t(`linkgrabber.candidate.state.${state}`), value: state }))
])
/** Manual sort keeps the stored package order so drag & drop stays visible. */
// Candidates are bucketed by package in one pass; filtering the whole list once per package
// was O(packages x candidates) and re-ran on every refresh of a list that can hold thousands.
const candidatesByPackage = computed(() => {
  const buckets = new Map<string, LinkCandidate[]>()
  for (const candidate of collector.candidates) {
    if (!candidate.package_id) continue
    if (stateFilter.value !== 'all' && candidate.state !== stateFilter.value) continue
    const bucket = buckets.get(candidate.package_id)
    if (bucket) bucket.push(candidate)
    else buckets.set(candidate.package_id, [candidate])
  }
  // The facets act on whole mirror groups, so they are applied once the bucket is complete: a
  // per-candidate filter would take a 720p mirror out of a group that is being kept for its
  // 1080p one, and the group would then be missing a member nobody asked to hide.
  // Hidden hosters act on the same whole groups: a group stays while any member is at a shown
  // hoster, and its hidden members then go along as the fallbacks they are (RD-130-21).
  for (const [id, bucket] of buckets) {
    const kept = new Set(hideHosters(filterRows(mirrorRows(bucket), collector.mirrorPreference), hiddenHosters.hidden.value)
      .flatMap(row => row.kind === 'link' ? [row.candidate.id] : row.group.members.map(member => member.id)))
    if (kept.size !== bucket.length) buckets.set(id, bucket.filter(candidate => kept.has(candidate.id)))
  }
  return buckets
})
const groups = computed<CollectorEntry[]>(() => sortCollectorEntries(collector.packages.map(pkg => ({
  kind: 'collector' as const,
  id: pkg.id,
  createdAt: pkg.created_at,
  position: pkg.position,
  package: pkg,
  candidates: sortCandidates(
    candidatesByPackage.value.get(pkg.id) ?? [],
    sort.value,
    descending.value
  )
})).filter(entry => entry.candidates.length), sort.value, descending.value))
const visibleLinks = computed(() => groups.value.reduce((count, group) => count + group.candidates.length, 0))
const nzbGroups = computed<NzbEntry[]>(() => nzb.imports
  .filter(item => item.state === 'imported' || item.state === 'failed')
  .map(item => ({ kind: 'nzb' as const, id: item.id, createdAt: item.created_at, position: item.position, item }))
  .sort((a, b) => a.position - b.position || a.createdAt.localeCompare(b.createdAt)))
// Non-manual sorts order the collector list by key, so the shared manual order would scramble it
// again; NZB imports (which have no hoster or size) then keep their own block at the end.
const entries = computed(() => sort.value === 'manual'
  ? mergeGrabberEntries(groups.value, nzbGroups.value)
  : [...groups.value, ...nzbGroups.value])

/**
 * The LinkGrabber as one stream of rows (RD-106-12).
 *
 * Package header, then its links while it is open, with reviewed NZB imports interleaved as
 * single rows. Collapsing filters this stream instead of hiding children inside a package,
 * because only a flat sequence can be windowed — and a row's key has to survive the window
 * moving past it, or the focus and the selection do not.
 */
interface GrabberPackageRow extends VirtualRow { kind: 'package', entry: CollectorEntry }
interface GrabberCandidateRow extends VirtualRow {
  kind: 'candidate'
  entry: CollectorEntry
  candidate: LinkCandidate
  /** Set when this row stands for a whole mirror group rather than for one link. */
  group?: MirrorGroup
  /** Set when this row is one of a group's other mirrors. */
  member?: boolean
}
interface GrabberNzbRow extends VirtualRow { kind: 'nzb', entry: NzbEntry }
type GrabberRow = GrabberPackageRow | GrabberCandidateRow | GrabberNzbRow

/** Starting estimates only; the list measures what the rows really are once they are drawn. */
const PACKAGE_ROW_SIZE = 48
const LINK_ROW_SIZE = 44
const NZB_ROW_SIZE = 48

const openPackages = useOpenSections({ defaultOpen: true })

const rows = computed<GrabberRow[]>(() => {
  const result: GrabberRow[] = []
  for (const entry of entries.value) {
    if (entry.kind === 'nzb') {
      result.push({ key: `nzb:${entry.id}`, size: NZB_ROW_SIZE, class: 'pt-2', kind: 'nzb', entry })
      continue
    }
    result.push({ key: `package:${entry.id}`, size: PACKAGE_ROW_SIZE, class: 'pt-2', kind: 'package', entry })
    if (!openPackages.isOpen(entry.id)) continue
    // One row per mirror group instead of one per link: a release page offering the same
    // episode at five hosters is one decision, not five (RD-110-19).
    for (const row of mirrorRows(entry.candidates)) {
      if (row.kind === 'link') {
        result.push({ key: `link:${row.candidate.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: row.candidate })
        continue
      }
      const group = row.group
      result.push({ key: `link:${group.chosen.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: group.chosen, group })
      if (!openMirrors.isOpen(group.key)) continue
      for (const member of group.members) {
        if (member.id === group.chosen.id) continue
        result.push({ key: `mirror:${member.id}`, size: LINK_ROW_SIZE, kind: 'candidate', entry, candidate: member, member: true })
      }
    }
  }
  return result
})
/** The order a range selection follows: what is on screen, not what is in the store. */
const orderedSelectionKeys = computed(() => rows.value.flatMap((row) => {
  if (row.kind === 'nzb') return [grabberKey('nzb', row.entry.id)]
  // A mirror of a group is not a candidate of its own: the group is what gets queued.
  if (row.kind === 'candidate' && !row.member && isSelectableCandidate(row.candidate)) return [grabberKey('collector', row.candidate.id)]
  return []
}))
const selection = useGrabberSelection(entries, orderedSelectionKeys)

const grabberList = ref<{
  focusRow: (key: string) => Promise<boolean>
  revealRow: (key: string) => Promise<boolean>
} | null>(null)

const {
  draggingEntry, draggingCandidate,
  dropOnPackage, dropOnNzb, dropOnCandidate, moveCandidate, moveEntry
} = useGrabberReorder({ groups, nzbGroups, sort, filterActive, notice, list: grabberList })

/** See `DownloadsView`: a checkbox reports its value, not the event that produced it. */
const extendSelection = ref(false)
function noteModifier(event: MouseEvent | KeyboardEvent): void {
  extendSelection.value = event.shiftKey
}

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
  persistDisplayedOrder, enqueueAll, enqueuePackage, enqueueSelected, finishEnqueue, enqueueCandidate, visibleCandidateIds
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


/** Persists the displayed candidate order so the downloader receives exactly this sequence. */

async function removeSelected(): Promise<void> {
  const count = selection.count.value
  const confirmed = await confirm({ title: t('linkgrabber.confirm.remove_selected_title'), description: t('linkgrabber.confirm.remove_selected_description', { count }, count), confirmLabel: t('common.actions.delete'), confirmIcon: 'i-lucide-trash-2', destructive: true })
  if (!confirmed) return
  bulkBusy.value = true
  const nzbIds = [...selection.nzbIds.value]
  for (const id of selection.collectorIds.value) await collector.deleteCandidate(id)
  for (const id of nzbIds) await nzb.remove(id)
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

/** "Delete all" clears everything the list shows: collector links (torrents included) plus NZB imports. */
async function clearAll(): Promise<void> {
  const nzbIds = nzbGroups.value.map(entry => entry.id)
  const count = collector.candidates.length + nzbIds.length
  const confirmed = await confirm({ title: t('linkgrabber.confirm.clear_all_title'), description: t('linkgrabber.confirm.clear_all_description', { count }, count), confirmLabel: t('linkgrabber.actions.clear_all'), confirmIcon: 'i-lucide-list-x', destructive: true })
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

function setPostprocessLevel(ids: string[], level: PostprocessLevel | null): void {
  void collector.updatePackages(ids, { postprocessLevel: level })
}

function packagesOf(candidateIds: string[]): string[] {
  // A Set, not `.includes()` inside a filter: the bulk bar calls this with the whole selection.
  const wanted = new Set(candidateIds)
  return [...new Set(collector.candidates.filter(c => wanted.has(c.id)).map(c => c.package_id).filter((id): id is string => Boolean(id)))]
}

/** Clears every facet in one request, plus the state filter, which is view state only. */
async function clearFilters(): Promise<void> {
  stateFilter.value = 'all'
  if (qualityFilter.value === 'all' && languageFilter.value === 'all' && hosterFilter.value === 'all') return
  facetBusy.value = true
  await collector.setMirrorPreference({ quality: null, language: null, hoster: null, hidden_hosters: collector.mirrorPreference.hidden_hosters })
  facetBusy.value = false
}

function candidateLabel(candidate: LinkCandidate): string {
  return candidate.file_name || candidate.url
}
void candidateLabel
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
          <span class="numeric whitespace-nowrap text-xs text-muted">{{ t('common.units.package', { count: entries.length }, entries.length) }} · {{ filterActive ? t('linkgrabber.filter.count', { visible: visibleLinks, total: collector.candidates.length }) : t('common.units.link', { count: collector.candidates.length }, collector.candidates.length) }}</span>
          <UButton icon="i-lucide-refresh-cw" color="neutral" variant="ghost" :aria-label="t('common.actions.refresh')" @click="collector.refresh" />
        </template>
      </UDashboardToolbar>
    </template>

    <template #body>
      <div data-tour="grabber-body" class="flex w-full flex-col gap-4">
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
          @postprocess="(level) => setPostprocessLevel(packagesOf(selection.collectorIds.value), level)"
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
          @click.capture="noteModifier"
          @keydown.capture="noteModifier"
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
              @select="selection.setCollector"
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
              :selected="selection.collectorIdSet.value.has(row.candidate.id)"
              :busy="collector.enqueuingIds.has(row.candidate.id)"
              :mirror-group="row.group ?? null"
              :mirror-open="row.group ? openMirrors.isOpen(row.group.key) : false"
              :mirror-member="row.member === true"
              @toggle-mirror="openMirrors.toggle"
              @choose-mirror="(id: string, chosen: boolean) => void collector.chooseMirror(id, chosen)"
              @dissolve-mirror="dissolveMirror"
              @hide-hoster="(hoster: string) => void hiddenHosters.setHidden(hoster, true)"
              @select="(id, value) => selection.pickCollector(id, value, extendSelection)"
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
              @select="(id, value) => selection.pickNzb(id, value, extendSelection)"
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
