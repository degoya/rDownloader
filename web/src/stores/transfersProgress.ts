import { computed, type Ref } from 'vue'

import type { Download, DownloadState } from '@/api/types'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import type { StreamListener } from '@/composables/useEventStream'
import { payloadOf } from '@/utils/eventPayload'

import { ACTIVE_STATES } from './transfersShared'

/** How far one row is that a runner works on, as `/downloads/rates` carries it (RD-1480-18). */
export interface DownloadProgress {
  id: string
  state: DownloadState
  committed_bytes: string
  total_bytes?: string | null
}

/** What an event asks for: the moving figures alone, or the whole queue. */
type Reading = 'progress' | 'queue'

/** Both reads wait this long, so a burst of events becomes one. */
const DELAY_MS = 400
/** In a hidden tab at most one read per this span: nobody looks, only the tab's title moves. */
export const HIDDEN_SPACING_MS = 5_000

const AT_WORK: readonly string[] = ACTIVE_STATES

/**
 * The rows with the bytes the service reports, or `null` when nothing moved; `stale` says that
 * the list no longer is the service's: a row at work is missing on one side or shows another
 * state. Such a row is left as it is, since a state alone does not say what else changed with it.
 */
export function patchProgress(
  rows: readonly Download[],
  progress: readonly DownloadProgress[]
): { rows: Download[] | null, stale: boolean } {
  const entries = new Map(progress.map(entry => [entry.id, entry]))
  let moved = false
  let stale = false
  const next = rows.map((row) => {
    const entry = entries.get(row.id)
    if (!entry) {
      if (AT_WORK.includes(row.state)) stale = true
      return row
    }
    entries.delete(row.id)
    if (entry.state !== row.state) {
      stale = true
      return row
    }
    const total = entry.total_bytes ?? null
    if (row.committed_bytes === entry.committed_bytes && (row.total_bytes ?? null) === total) return row
    moved = true
    return { ...row, committed_bytes: entry.committed_bytes, total_bytes: total }
  })
  return { rows: moved ? next : null, stale: stale || entries.size > 0 }
}

interface QueueFollowerContext {
  downloads: Readonly<Ref<readonly Download[]>>
  /** Reads the whole queue. */
  refresh: () => Promise<void>
  /** Reads the moving figures; answers whether the list turned out to be stale. */
  refreshProgress: () => Promise<boolean>
  /** True while a read is on its way. */
  reading: () => boolean
  /** The last read of the queue failed, so what is shown may be anything. */
  failed: () => boolean
  /** Whether a hidden tab still needs this reading (the tab title, desktop notifications). */
  neededHidden: (reading: Reading) => boolean
  /** Other events of the same subscription, patched in place. */
  handlers: Record<string, StreamListener>
}

/**
 * Follows the queue by its events (RD-1480-18).
 *
 * Every `download.progress` used to read the whole queue again, twice a second and 3.6 MB each
 * time with 5000 files. Progress reads the figures of the rows at work now; the queue is read on
 * what changes a row otherwise: a state, a package, a hole in the stream, a figure that does not
 * fit the list, a progress event about a row the figures do not cover. A hidden tab reads nothing
 * it does not need and catches up when it is looked at again.
 */
export function createQueueFollower(context: QueueFollowerContext) {
  let wanted: Reading | null = null
  let lastRead = 0

  /** The rows `/downloads/rates` covers, by their id and, for Usenet, their file's. */
  const atWork = computed(() => {
    const ids = new Set<string>()
    for (const row of context.downloads.value) {
      if (!AT_WORK.includes(row.state)) continue
      ids.add(row.id)
      if (row.nzb_file_id) ids.add(row.nzb_file_id)
    }
    return ids
  })

  const hidden = () => document.visibilityState === 'hidden'

  /** The reading due now: what is wanted, or in a hidden tab the part of it that is needed. */
  function due(): Reading | null {
    if (wanted === null) return null
    if (!hidden()) return wanted
    if (wanted === 'queue' && context.neededHidden('queue')) return 'queue'
    return context.neededHidden('progress') ? 'progress' : null
  }

  async function read(): Promise<void> {
    const reading = due()
    if (reading === null) return
    lastRead = Date.now()
    if (reading === 'queue') {
      wanted = null
      await context.refresh()
      return
    }
    // A hidden tab that reads the figures alone still owes the queue it was asked for.
    if (wanted === 'progress') wanted = null
    const stale = await context.refreshProgress()
    // Already owed (a hidden tab): asking again would only read the figures over and over.
    if (stale && wanted !== 'queue') want('queue')
  }

  const events = debouncedEventRefresh([], read, {
    delayMs: DELAY_MS,
    busy: () => context.reading() || (hidden() && Date.now() - lastRead < HIDDEN_SPACING_MS),
    handlers: {
      'download.progress': (event) => {
        const { download_id: id, recording } = payloadOf(event)
        // A stream marker names no row, a recording changes more of its row than the bytes, and
        // the figures leave out a row at rest — one paused while its last bytes were written.
        wantFor(id, recording === true)
      },
      'usenet.changed': (event) => {
        const payload = payloadOf(event)
        // A Usenet transfer's progress: an article's state is no figure of the queue's, the
        // bytes written are. Everything else Usenet announces may change rows.
        if (typeof payload.nzb_segment_id === 'string') want('progress')
        else wantFor(payload.resource === 'nzb_checkpoint' ? payload.nzb_file_id : undefined)
      },
      'download.state': (event) => {
        // A torrent's own state (trackers, plan, seed clock) is read by its details; no row of the
        // queue carries any of it. An event that names a state as well is a transition.
        const payload = payloadOf(event)
        if (payload.torrent !== true || payload.state !== undefined) want('queue')
      },
      'package.state': () => want('queue'),
      // Deleting one of these clears the reference in the rows that named it.
      'category.changed': () => want('queue'),
      'account.changed': () => want('queue'),
      'auth_profile.changed': () => want('queue'),
      'remote_credential.changed': () => want('queue'),
      ...context.handlers
    }
  })

  function wantFor(id: unknown, whole = false): void {
    want(!whole && typeof id === 'string' && atWork.value.has(id) ? 'progress' : 'queue')
  }

  function want(reading: Reading): void {
    // What is shown after a failed read may be anything, so any event asks for all of it.
    if (wanted !== 'queue') wanted = context.failed() ? 'queue' : reading
    if (due() !== null) events.schedule()
  }

  function visibilityChanged(): void {
    if (hidden() || wanted === null) return
    if (context.reading()) events.schedule()
    else void read()
  }

  return {
    connect(): void {
      events.connect()
      document.addEventListener('visibilitychange', visibilityChanged)
    },
    disconnect(): void {
      events.disconnect()
      document.removeEventListener('visibilitychange', visibilityChanged)
      wanted = null
    },
    /** Whether the moving figures are kept current right now; false only in a hidden tab. */
    followsProgress: () => !hidden() || context.neededHidden('progress')
  }
}
