import { reactive } from 'vue'

import { lookupDuplicates } from '@/api/storage'

/**
 * Which LinkGrabber addresses are already in the queue, by source identity (RD-150-01).
 *
 * The LinkGrabber marks a link that repeats one of its own as `duplicate`; what it cannot see is
 * the queue. This asks the service once per change of the address list — a magnet with other
 * trackers, an `http` spelling of an `https` link, a hoster alias all count as the same source —
 * and every row reads the answer from here rather than asking on its own.
 */
const queued = reactive(new Map<string, number>())
/** The service takes at most this many addresses per request. */
const BATCH = 500
let generation = 0

export async function refreshQueuedSources(urls: readonly string[]): Promise<void> {
  const current = ++generation
  const unique = [...new Set(urls)]
  const found = new Map<string, number>()
  for (let start = 0; start < unique.length; start += BATCH) {
    const answer = await lookupDuplicates(unique.slice(start, start + BATCH))
    // A newer list is on its way; this answer describes one that no longer exists.
    if (current !== generation) return
    if (!answer.ok) return
    for (const entry of answer.data) {
      if (entry.queue.length) found.set(entry.url, entry.queue.length)
    }
  }
  queued.clear()
  for (const [url, count] of found) queued.set(url, count)
}

/** How many queue downloads ask for the same source as `url`; 0 when none or not known yet. */
export function queuedCount(url: string): number {
  return queued.get(url) ?? 0
}
