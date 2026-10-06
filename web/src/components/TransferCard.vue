<script setup lang="ts">
import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Download, DownloadSourcesResponse, TorrentPlanRequest } from '@/api/types'
import DragHandle from '@/components/DragHandle.vue'
import TorrentFileTree from '@/components/TorrentFileTree.vue'
import TorrentMoveModal from '@/components/TorrentMoveModal.vue'
import TorrentPeerList from '@/components/TorrentPeerList.vue'
import TorrentSeedingPolicy from '@/components/TorrentSeedingPolicy.vue'
import TorrentTrackerList from '@/components/TorrentTrackerList.vue'
import TransferSourceList from '@/components/TransferSourceList.vue'
import { scopeLabel, useAuthProfileSelector } from '@/composables/useAuthProfiles'
import { useErrorToast } from '@/composables/useErrorToast'
import { useStagedResolvers } from '@/composables/useStagedResolvers'
import { useTorrentsStore } from '@/stores/torrents'
import { RESETTABLE_STATES } from '@/stores/transfers'
import { translateServerMessage } from '@/i18n/server'
import { formatByteProgress, formatBytes, formatDuration, formatPauseEnd, formatRate, progressOf, stateColor, stateLabel } from '@/utils/format'
import { sourcePageUrl } from '@/utils/sourcePage'

const props = defineProps<{ download: Download, bytesPerSecond?: number, etaSeconds?: number | null, destination?: string, accountLabel?: string | null, selected?: boolean }>()
const emit = defineEmits<{
  pause: [id: string]
  resume: [id: string]
  cancel: [id: string]
  stopSeeding: [id: string]
  remove: [id: string]
  reset: [id: string]
  rename: [id: string]
  select: [id: string, selected: boolean]
  copyPath: [path: string]
  /** The row's address onto the clipboard; the view copies and says so (RD-190-21). */
  copyLinks: [links: string[]]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the file up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
}>()

const { t } = useI18n()
const expanded = ref(false)
/** HTTP download running without any provider account — a free/direct fetch of the source URL. */
const { usable: authProfiles, fromSelection, assign } = useAuthProfileSelector()
const authProfileOverride = ref<string | null>(null)
/** The stored selection, unless this card just changed it. */
const authProfileValue = computed(
  () => authProfileOverride.value ?? fromSelection(props.download.auth_profile)
)
const authProfileItems = computed(() => [
  { label: t('downloads.transfer.auth_profile_auto'), value: 'auto' },
  { label: t('downloads.transfer.auth_profile_none'), value: 'none' },
  ...authProfiles.value.map(profile => ({ label: `${profile.name} - ${scopeLabel(profile)}`, value: profile.id }))
])

async function assignAuthProfile(value: string): Promise<void> {
  authProfileOverride.value = value
  // On failure fall back to what the server still has, rather than showing a change that
  // did not happen.
  if (!(await assign(props.download.id, value))) authProfileOverride.value = null
}

const freeDownload = computed(() => !props.accountLabel && !props.download.account_id && props.download.kind === 'http')
const active = computed(() => ['resolving', 'downloading', 'verifying', 'repairing', 'extracting', 'seeding'].includes(props.download.state))
const renamable = computed(() => ['queued', 'paused', 'retry_wait', 'failed', 'blocked', 'cancelled'].includes(props.download.state))
const sizeLabel = computed(() => formatByteProgress(props.download.committed_bytes, props.download.total_bytes))
/**
 * The remaining time, or nothing at all. The server sends no figure while the size is unknown,
 * the transfer is paused or the rate has fallen to zero, and nothing is what is shown then —
 * no placeholder, no infinity.
 */
const etaLabel = computed(() => formatDuration(props.etaSeconds))
const lastError = computed(() => props.download.last_error ? translateServerMessage(props.download.last_error) : null)
/**
 * When the file is tried again on its own (RD-191-12): the retry after a failure or a hoster's
 * limit, and the round of the automatic retry a failed file waits for.
 */
const nextAttempt = computed(() => ['retry_wait', 'failed'].includes(props.download.state) ? formatPauseEnd(props.download.next_retry_at) : '')
const pausable = computed(() => ['downloading', 'resolving', 'queued', 'retry_wait'].includes(props.download.state))
const resumable = computed(() => ['paused', 'failed', 'blocked', 'cancelled', 'skipped'].includes(props.download.state))
const cancelled = computed(() => props.download.state === 'cancelled')
/** All row actions live in one menu so the file name keeps the width. */
const recording = computed(() => props.download.kind === 'record')
const resettable = computed(() => RESETTABLE_STATES.includes(props.download.state))
/** The page a media file was extracted from — the only source page a queued file knows. */
const sourcePage = computed(() => sourcePageUrl(props.download.media?.page_url))

/** Torrent detail: the file tree and the tracker list, both loaded when first opened. */
const torrents = useTorrentsStore()
const isTorrent = computed(() => props.download.kind === 'torrent')
const torrentDetail = computed(() => torrents.detail('download', props.download.id))
const torrentTrackers = computed(() => torrents.trackersOf(props.download.id))
const torrentBusy = computed(() => torrents.isBusy('download', props.download.id))
const torrentError = computed(() => torrents.errorOf('download', props.download.id))
type TorrentTab = 'files' | 'trackers' | 'peers' | 'seeding'
const TORRENT_TABS: TorrentTab[] = ['files', 'trackers', 'peers', 'seeding']
const torrentTab = ref<TorrentTab>('files')
const torrentTabItems = computed(() => TORRENT_TABS.map(value => ({ label: t(`torrent.tabs.${value}`), value })))
const torrentStats = computed(() => torrents.statsOf(props.download.id))
const torrentPeers = computed(() => torrents.peersOf(props.download.id))
const torrentPieces = computed(() => torrents.piecesOf(props.download.id))
const torrentSeeding = computed(() => torrents.seedingOf(props.download.id))

/**
 * Recheck and change location (RD-1100-10). Both are refused while a move runs; a move is offered
 * for a seed and a paused torrent, the two whose files no runner is writing.
 */
const relocating = computed(() => Boolean(torrentDetail.value?.relocation))
const recheckable = computed(() => isTorrent.value && !relocating.value
  && ['queued', 'paused', 'retry_wait', 'failed', 'blocked', 'cancelled', 'resolving', 'downloading', 'seeding'].includes(props.download.state))
const movable = computed(() => isTorrent.value && !relocating.value && ['seeding', 'paused'].includes(props.download.state))
const checkingPercent = computed(() => {
  const sample = torrentStats.value
  if (!sample?.checking) return null
  const total = Number(sample.total_bytes)
  return total > 0 ? Math.min(100, Math.round((Number(sample.progress_bytes) / total) * 100)) : 0
})
const recheckLabel = computed(() => {
  const recheck = torrentDetail.value?.recheck
  if (!recheck) return ''
  if (!recheck.finished_at) return t('torrent.recheck.pending')
  if (recheck.verified_bytes >= recheck.total_bytes) return t('torrent.recheck.complete')
  return t('torrent.recheck.incomplete', {
    verified: formatBytes(String(recheck.verified_bytes)),
    total: formatBytes(String(recheck.total_bytes))
  })
})
// The check's result is written once it has run, so the detail is read again when it ends.
watch(() => torrentStats.value?.checking, (now, before) => {
  if (before && !now && torrentDetail.value) void torrents.load('download', props.download.id)
})

/** Peers and pieces are pulled, and only while the tab is actually open. */
async function openTab(tab: TorrentTab): Promise<void> {
  torrentTab.value = tab
  if (tab === 'peers') {
    await Promise.all([
      torrents.loadPeers(props.download.id),
      torrents.loadPieces(props.download.id)
    ])
  } else if (tab === 'seeding') {
    await torrents.loadSeeding(props.download.id)
  }
}

/**
 * The mirrors of a Metalink download (RD-150-03), read again at every opening: their health
 * changes while the transfer runs. A download with a single address answers with none.
 */
const sources = ref<DownloadSourcesResponse | null>(null)

async function loadSources(): Promise<void> {
  try {
    const response = await api.GET('/api/v1/downloads/{id}/sources', {
      params: { path: { id: props.download.id } }
    })
    const data = response.data as DownloadSourcesResponse | undefined
    sources.value = data && Array.isArray(data.sources) ? data : null
  } catch {
    // The list is extra detail; the card works without it.
    sources.value = null
  }
}

async function openDetails(): Promise<void> {
  expanded.value = !expanded.value
  if (expanded.value && props.download.kind === 'http') await loadSources()
  if (!expanded.value || !isTorrent.value || torrentDetail.value) return
  await Promise.all([
    torrents.load('download', props.download.id),
    torrents.loadTrackers(props.download.id)
  ])
}

/**
 * "Test with new version" (RD-140-02): offered only while a staged resolver version is loaded
 * that would handle this download, and only while the download is not running — a running job
 * keeps the version it started with, and a finished one does not start again.
 */
const toast = useToast()
const showError = useErrorToast()
const { stagedFor, trial } = useStagedResolvers()
const trialResolver = computed(() => active.value || props.download.state === 'completed'
  ? null
  : stagedFor(props.download.source))

async function tryStaged(): Promise<void> {
  const resolver = trialResolver.value
  if (!resolver) return
  const outcome = await trial(resolver, props.download.id)
  if (outcome.error) showError(outcome.error)
  else toast.add({ title: outcome.message ?? '', color: 'success', icon: 'i-lucide-flask-conical' })
}

const moveModal = useOverlay().create(TorrentMoveModal)

async function recheckTorrent(): Promise<void> {
  const outcome = await torrents.recheck(props.download.id)
  if (outcome.error) showError(outcome.error)
  else toast.add({ title: outcome.message ?? '', color: 'success', icon: 'i-lucide-scan-search' })
}

async function moveTorrent(): Promise<void> {
  const message = await moveModal.open({ downloadId: props.download.id }).result
  if (typeof message === 'string' && message) toast.add({ title: message, color: 'success', icon: 'i-lucide-folder-input' })
}

function saveTorrentPlan(plan: TorrentPlanRequest): void {
  void torrents.savePlan('download', props.download.id, plan)
}
const actions = computed(() => [[
  ...(pausable.value
    ? [{
        // Stopping a recording finalizes the file and completes the row.
        label: recording.value ? t('downloads.transfer.stop_recording') : t('common.actions.pause'),
        icon: recording.value ? 'i-lucide-square' : 'i-lucide-pause',
        onSelect: () => emit('pause', props.download.id)
      }]
    : []),
  ...(resumable.value
    ? [{
        label: cancelled.value ? t('downloads.transfer.resume_cancelled_aria') : t('common.actions.start'),
        icon: 'i-lucide-play',
        onSelect: () => emit('resume', props.download.id)
      }]
    : []),
  ...(trialResolver.value
    ? [{
        label: t('downloads.transfer.trial_staged', { name: trialResolver.value.name, version: trialResolver.value.version }),
        icon: 'i-lucide-flask-conical',
        onSelect: () => void tryStaged()
      }]
    : []),
  ...(props.download.state === 'seeding'
    ? [{ label: t('downloads.transfer.stop_seeding'), icon: 'i-lucide-square', onSelect: () => emit('stopSeeding', props.download.id) }]
    : []),
  ...(recheckable.value
    ? [{ label: t('torrent.actions.recheck'), icon: 'i-lucide-scan-search', onSelect: () => void recheckTorrent() }]
    : []),
  ...(movable.value
    ? [{ label: t('torrent.actions.move'), icon: 'i-lucide-folder-input', onSelect: () => void moveTorrent() }]
    : []),
  ...(renamable.value
    ? [{ label: t('common.actions.rename'), icon: 'i-lucide-pencil', onSelect: () => emit('rename', props.download.id) }]
    : [])
], [
  { label: t('common.actions.copy_link'), icon: 'i-lucide-link', onSelect: () => emit('copyLinks', [props.download.source]) },
  ...(sourcePage.value
    ? [{ label: t('common.actions.open_source_page'), icon: 'i-lucide-external-link', to: sourcePage.value, target: '_blank' }]
    : [])
], [
  ...(!['completed', 'cancelled', 'seeding'].includes(props.download.state)
    ? [{ label: t('common.actions.cancel'), icon: 'i-lucide-x', color: 'error' as const, onSelect: () => emit('cancel', props.download.id) }]
    : []),
  ...(resettable.value
    ? [{ label: t('downloads.transfer.reset'), icon: 'i-lucide-rotate-ccw', color: 'error' as const, onSelect: () => emit('reset', props.download.id) }]
    : []),
  ...(!active.value
    ? [{ label: t('downloads.transfer.remove_aria'), icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => emit('remove', props.download.id) }]
    : [])
]].filter(group => group.length > 0))
const kindIcon = computed(() => {
  if (props.download.kind === 'usenet') return 'i-lucide-radio-tower'
  if (props.download.kind === 'media') return props.download.media?.kind === 'audio' ? 'i-lucide-music' : 'i-lucide-clapperboard'
  if (props.download.kind === 'gallery') return 'i-lucide-images'
  if (props.download.kind === 'record') return 'i-lucide-radio'
  if (props.download.kind === 'torrent') return 'i-lucide-magnet'
  if (props.download.kind === 'ftp') return 'i-lucide-folder-symlink'
  if (props.download.kind === 'sftp') return 'i-lucide-shield'
  if (props.download.kind === 'plugin') return 'i-lucide-blocks'
  if (props.download.kind === 'object_storage') return 'i-lucide-cylinder'
  return 'i-lucide-file-down'
})
/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('downloads.transfer.drag_title')} — ${t('common.a11y.reorder_keys')}`)
</script>

<template>
  <article
    class="group bg-default transition hover:bg-elevated/60"
    :class="props.selected ? 'bg-primary/5' : ''"
    @dragover.prevent
    @drop.prevent.stop="emit('drop', props.download.id)"
  >
    <div class="queue-row px-2 py-1.5">
      <DragHandle
        class="queue-cell-handle grid place-items-center"
        :label="dragTitle"
        @dragstart="emit('dragstart', props.download.id)"
        @move="(delta: -1 | 1) => emit('move', props.download.id, delta)"
      />
      <UCheckbox class="queue-cell-select justify-self-center" :model-value="props.selected ?? false" :aria-label="t('downloads.transfer.select_aria')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', props.download.id, value === true)" />
      <UButton
        class="queue-cell-expand"
        :icon="expanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        size="xs"
        color="neutral"
        variant="ghost"
        :aria-label="expanded ? t('downloads.transfer.hide_details') : t('downloads.transfer.show_details')"
        @click="openDetails"
      />
      <span class="queue-cell-name flex min-w-0 items-center gap-1.5">
        <UIcon :name="kindIcon" class="size-4 shrink-0 text-primary" />
        <span class="min-w-0 truncate text-sm text-highlighted" :title="props.download.file_name">{{ props.download.file_name }}</span>
      </span>
      <span class="queue-cell-state min-w-0"><UBadge :color="stateColor(props.download.state)" variant="subtle" size="sm" class="max-w-full truncate">{{ stateLabel(props.download.state, props.download) }}</UBadge></span>
      <!-- A full bar already says 100%; the number beside it is the same statement twice. -->
      <div class="queue-cell-progress items-center gap-1.5">
        <UProgress :model-value="progressOf(props.download)" size="2xs" class="flex-1" />
        <span v-if="progressOf(props.download) < 100" class="numeric w-8 text-right text-2xs text-toned">{{ progressOf(props.download).toFixed(0) }}%</span>
      </div>
      <span class="queue-cell-size min-w-0 text-right">
        <span class="numeric block truncate text-xs text-muted">{{ sizeLabel }}</span>
        <span v-if="props.download.state === 'downloading'" class="numeric block truncate text-2xs font-medium text-primary" :aria-label="t('downloads.transfer.rate_aria', { rate: formatRate(props.bytesPerSecond) })">{{ formatRate(props.bytesPerSecond) }}<span v-if="etaLabel" class="text-toned" :aria-label="t('downloads.transfer.eta_aria', { duration: etaLabel })"> · {{ etaLabel }}</span></span>
      </span>
      <span class="queue-cell-meta min-w-0 truncate text-xs text-muted" :title="props.accountLabel ?? undefined">{{ props.accountLabel ?? '' }}</span>
      <div class="queue-cell-actions flex items-center justify-end opacity-70 transition group-hover:opacity-100">
        <UDropdownMenu v-if="actions.length" :items="actions" :content="{ align: 'end' }">
          <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('downloads.transfer.actions_aria')" :title="t('downloads.transfer.actions_aria')" />
        </UDropdownMenu>
      </div>
    </div>
    <p v-if="lastError && !expanded" class="truncate px-11 pb-1.5 text-xs text-error" :title="lastError">{{ lastError }}</p>
    <p v-if="nextAttempt" class="flex items-center gap-1 px-11 pb-1.5 text-xs text-muted" data-testid="next-attempt">
      <UIcon name="i-lucide-clock" class="size-3.5 shrink-0" />
      <span class="numeric">{{ t('downloads.transfer.next_attempt', { time: nextAttempt }) }}</span>
    </p>
    <div v-if="expanded" class="grid gap-1 border-t border-muted px-11 py-2 text-xs text-muted">
      <div class="flex items-center gap-2 md:hidden">
        <UProgress :model-value="progressOf(props.download)" size="xs" class="flex-1" />
        <span class="numeric w-9 text-right text-2xs text-toned">{{ progressOf(props.download).toFixed(0) }}%</span>
      </div>
      <p class="min-w-0 truncate font-mono" :title="props.download.source">{{ props.download.source }}</p>
      <TransferSourceList v-if="sources" :sources="sources" />
      <USeparator v-if="isTorrent" class="mb-1" />
      <div v-if="isTorrent" class="grid gap-2">
        <URadioGroup
          :model-value="torrentTab"
          :items="torrentTabItems"
          variant="card"
          indicator="hidden"
          orientation="horizontal"
          size="xs"
          :aria-label="t('torrent.tabs.label')"
          @update:model-value="openTab"
        />
        <p v-if="torrentStats" class="flex flex-wrap items-center gap-3 text-xs text-muted">
          <span>{{ t('torrent.stats.ratio') }} <span class="numeric text-toned">{{ torrentStats.ratio.toFixed(2) }}</span></span>
          <span>{{ t('torrent.stats.uploaded') }} <span class="numeric text-toned">{{ formatByteProgress(torrentStats.uploaded_bytes, null) }}</span></span>
          <span>{{ t('torrent.stats.peers') }} <span class="numeric text-toned">{{ torrentStats.peer_count }}</span></span>
          <span v-if="!torrentStats.live" class="text-warning">{{ t('torrent.stats.offline') }}</span>
          <span v-if="checkingPercent !== null" class="numeric text-info">{{ t('torrent.recheck.checking', { percent: checkingPercent }) }}</span>
        </p>
        <p v-if="recheckLabel" class="flex items-center gap-1 text-xs text-muted">
          <UIcon name="i-lucide-scan-search" class="size-3.5 shrink-0" />
          <span>{{ recheckLabel }}</span>
        </p>
        <p v-if="torrentDetail?.relocation" class="flex min-w-0 items-center gap-1 text-xs text-info">
          <UIcon name="i-lucide-folder-input" class="size-3.5 shrink-0" />
          <span class="min-w-0 truncate font-mono" :title="torrentDetail.relocation.to">{{ t('torrent.move.running', { path: torrentDetail.relocation.to }) }}</span>
        </p>
        <p v-else-if="torrentDetail?.relocation_error" class="text-xs text-error">{{ t('torrent.move.taken_back', { reason: torrentDetail.relocation_error }) }}</p>
        <p v-if="torrentError" class="text-xs text-error">{{ torrentError }}</p>
        <template v-if="torrentTab === 'files'">
          <TorrentFileTree
            v-if="torrentDetail?.plan"
            :plan="torrentDetail.plan"
            :capabilities="torrentDetail.capabilities"
            :busy="torrentBusy"
            @change="saveTorrentPlan"
          />
          <p v-else class="text-xs text-muted">{{ t('torrent.tree.empty') }}</p>
          <USeparator v-if="torrentDetail?.web_seeds?.length" />
          <div v-if="torrentDetail?.web_seeds?.length" class="grid gap-1">
            <p class="text-xs text-toned">{{ t('torrent.web_seeds.title') }}</p>
            <p class="text-xs text-muted">{{ t('torrent.web_seeds.unsupported') }}</p>
            <p
              v-for="seed in torrentDetail.web_seeds"
              :key="seed"
              class="min-w-0 truncate font-mono text-xs text-muted"
              :title="seed"
            >{{ seed }}</p>
          </div>
        </template>
        <TorrentPeerList
          v-else-if="torrentTab === 'peers'"
          :peers="torrentPeers"
          :pieces="torrentPieces"
          @more="(cursor) => torrents.loadPeers(props.download.id, cursor)"
        />
        <TorrentSeedingPolicy
          v-else-if="torrentTab === 'seeding' && torrentSeeding"
          :policy="torrentSeeding"
          :busy="torrentBusy"
          @save="(policy) => torrents.saveSeeding(props.download.id, policy)"
          @clear="torrents.clearSeeding(props.download.id)"
        />
        <TorrentTrackerList
          v-else-if="torrentTrackers && torrentTab === 'trackers'"
          :trackers="torrentTrackers"
          :busy="torrentBusy"
          @save="(entries) => torrents.saveTrackers(props.download.id, entries)"
          @reannounce="torrents.reannounce(props.download.id)"
          @scrape="torrents.scrapeTrackers(props.download.id)"
        />
      </div>
      <p v-if="props.download.media" class="flex min-w-0 items-center gap-1.5">
        <UIcon :name="kindIcon" class="size-3.5 shrink-0" />
        <span class="shrink-0">{{ t('downloads.media.variant') }} <span class="font-mono text-toned">{{ props.download.media.variant_id }}</span></span>
        <ULink v-if="sourcePage" :to="sourcePage" target="_blank" rel="noopener noreferrer" class="min-w-0 truncate font-mono hover:underline" :title="t('downloads.media.open_page')">{{ props.download.media.page_url }}</ULink>
        <span v-else class="min-w-0 truncate font-mono">{{ props.download.media.page_url }}</span>
      </p>
      <p v-if="props.destination" class="flex min-w-0 items-center gap-1 font-mono" :title="props.destination">
        <UIcon name="i-lucide-folder" class="size-3.5 shrink-0" />
        <span class="min-w-0 truncate">{{ props.destination }}</span>
        <UButton icon="i-lucide-copy" size="xs" color="neutral" variant="ghost" :aria-label="t('downloads.package.copy_path_aria')" @click="emit('copyPath', props.destination)" />
      </p>
      <div class="flex flex-wrap items-center gap-x-4 gap-y-1">
        <span class="numeric">{{ sizeLabel }}</span>
        <span v-if="props.download.state === 'downloading'" class="numeric font-medium text-primary">{{ formatRate(props.bytesPerSecond) }}</span>
        <span v-if="etaLabel">{{ t('downloads.transfer.eta') }} <span class="numeric text-toned">{{ etaLabel }}</span></span>
        <UBadge v-if="props.accountLabel" color="primary" variant="outline" size="sm" icon="i-lucide-badge-check">{{ props.accountLabel }}</UBadge>
        <UBadge v-else-if="freeDownload" color="neutral" variant="outline" size="sm" icon="i-lucide-user-x" :title="t('downloads.free_download_hint')">{{ t('downloads.free_download') }}</UBadge>
        <span v-if="props.download.retry_count">{{ t('downloads.transfer.attempts') }} <span class="numeric">{{ props.download.retry_count }}</span></span>
        <span v-if="props.download.computed_checksum" class="font-mono">{{ props.download.computed_checksum.algorithm }} {{ props.download.computed_checksum.value }}</span>
      </div>
      <UFormField v-if="props.download.kind === 'http'" :label="t('downloads.transfer.auth_profile')" orientation="horizontal" size="xs" class="flex-wrap justify-start">
        <USelect
          :model-value="authProfileValue"
          :items="authProfileItems"
          size="xs"
          class="min-w-56"
          @update:model-value="assignAuthProfile"
        />
      </UFormField>
      <p v-if="lastError" class="leading-5 text-error">{{ lastError }}</p>
    </div>
  </article>
</template>
