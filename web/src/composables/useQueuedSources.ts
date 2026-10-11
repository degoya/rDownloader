import { reactive } from 'vue'

import { lookupDuplicates } from '@/api/storage'
import type { components } from '@/api/schema'

export type HistoryDuplicate = components['schemas']['HistoryDuplicate']

/**
 * Which LinkGrabber addresses are already in the queue, by source identity (RD-150-01).
 *
 * The LinkGrabber marks a link that repeats one of its own as `duplicate`; what it cannot see is
 * the queue. This asks the service once per change of the address list — a magnet with other
 * trackers, an `http` spelling of an `https` link, a hoster alias all count as the same source —
 * and every row reads the answer from here rather than asking on its own.
 * While the setting `duplicates_include_history` is on, the same answer names the download
 * history's packages of that source as well (RD-1240-14); the newest of them is kept per address.
 */
const queued = reactive(new Map<string, number>())
const downloaded = reactive(new Map<string, HistoryDuplicate>())
/** The service takes at most this many addresses per request. */
const BATCH = 500
let generation = 0

export async function refreshQueuedSources(urls: readonly string[]): Promise<void> {
  const current = ++generation
  const unique = [...new Set(urls)]
  const found = new Map<string, number>()
  const earlier = new Map<string, HistoryDuplicate>()
  for (let start = 0; start < unique.length; start += BATCH) {
    const answer = await lookupDuplicates(unique.slice(start, start + BATCH))
    // A newer list is on its way; this answer describes one that no longer exists.
    if (current !== generation) return
    if (!answer.ok) return
    for (const entry of answer.data) {
      if (entry.queue.length) found.set(entry.url, entry.queue.length)
      const [newest] = entry.history
      if (newest) earlier.set(entry.url, newest)
    }
  }
  queued.clear()
  for (const [url, count] of found) queued.set(url, count)
  downloaded.clear()
  for (const [url, entry] of earlier) downloaded.set(url, entry)
}

/** How many queue downloads ask for the same source as `url`; 0 when none or not known yet. */
export function queuedCount(url: string): number {
  return queued.get(url) ?? 0
}

/** The newest history package of the same source as `url`, or `null` (RD-1240-14). */
export function downloadedBefore(url: string): HistoryDuplicate | null {
  return downloaded.get(url) ?? null
}
