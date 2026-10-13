import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Download, TorrentPlanRequest } from '@/api/types'
import { useTorrentsStore } from '@/stores/torrents'
import { formatBytes } from '@/utils/format'

type TorrentTab = 'files' | 'trackers' | 'peers' | 'seeding'
const TORRENT_TABS: TorrentTab[] = ['files', 'trackers', 'peers', 'seeding']

/**
 * A queue row's torrent detail (RD-1100-10): the file tree and the tracker list, both loaded when
 * the row first opens, peers, pieces and seeding while their tab is open, and the state of a
 * recheck or a move.
 */
export function useTransferTorrent(download: () => Download) {
  const { t } = useI18n()
  const torrents = useTorrentsStore()
  const isTorrent = computed(() => download().kind === 'torrent')
  const torrentDetail = computed(() => torrents.detail('download', download().id))
  const torrentTrackers = computed(() => torrents.trackersOf(download().id))
  const torrentBusy = computed(() => torrents.isBusy('download', download().id))
  const torrentError = computed(() => torrents.errorOf('download', download().id))
  const torrentTab = ref<TorrentTab>('files')
  const torrentTabItems = computed(() => TORRENT_TABS.map(value => ({ label: t(`torrent.tabs.${value}`), value })))
  const torrentStats = computed(() => torrents.statsOf(download().id))
  const torrentPeers = computed(() => torrents.peersOf(download().id))
  const torrentPieces = computed(() => torrents.piecesOf(download().id))
  const torrentSeeding = computed(() => torrents.seedingOf(download().id))

  /**
   * Recheck and change location (RD-1100-10). Both are refused while a move runs; a move is offered
   * for a seed and a paused torrent, the two whose files no runner is writing.
   */
  const relocating = computed(() => Boolean(torrentDetail.value?.relocation))
  const recheckable = computed(() => isTorrent.value && !relocating.value
    && ['queued', 'paused', 'retry_wait', 'failed', 'blocked', 'cancelled', 'resolving', 'downloading', 'seeding'].includes(download().state))
  const movable = computed(() => isTorrent.value && !relocating.value && ['seeding', 'paused'].includes(download().state))
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
    if (before && !now && torrentDetail.value) void torrents.load('download', download().id)
  })

  /** Peers and pieces are pulled, and only while the tab is actually open. */
  async function openTab(tab: TorrentTab): Promise<void> {
    torrentTab.value = tab
    if (tab === 'peers') {
      await Promise.all([
        torrents.loadPeers(download().id),
        torrents.loadPieces(download().id)
      ])
    } else if (tab === 'seeding') {
      await torrents.loadSeeding(download().id)
    }
  }

  function saveTorrentPlan(plan: TorrentPlanRequest): void {
    void torrents.savePlan('download', download().id, plan)
  }

  return {
    isTorrent, torrentDetail, torrentTrackers, torrentBusy, torrentError, torrentTab, torrentTabItems,
    torrentStats, torrentPeers, torrentPieces, torrentSeeding, recheckable, movable, checkingPercent, recheckLabel,
    openTab, saveTorrentPlan
  }
}
