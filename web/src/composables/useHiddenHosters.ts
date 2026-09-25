import { computed, ref } from 'vue'

import type { LinkCandidate } from '@/api/types'
import { useCollectorStore } from '@/stores/collector'
import { hosterOf } from '@/utils/collectorSort'
import { hideHosters, mirrorRows } from '@/utils/mirrorGroups'

/** One hoster of the LinkGrabber, as its quick filter lists it. */
export interface HosterCount {
  hoster: string
  /** Every link of this hoster in the LinkGrabber, whatever else filters the list. */
  count: number
  hidden: boolean
}

/**
 * Hosters the LinkGrabber hides (RD-130-21), after JDownloader's quick filter.
 *
 * Several at once, and independent of the hoster facet, which shows *one*. The list is part of
 * the standing mirror preference, so it is stored on the server and outlives a reload and a
 * restart; this holds nothing of its own but the flag of a write in flight.
 *
 * What counts as hidden is decided per mirror group, the way `hideHosters` draws it: a hidden
 * hoster's link that is a mirror of a shown one stays, as that link's fallback. Only the links
 * nothing shown stands for are the ones the summary counts and the queue leaves behind.
 */
export function useHiddenHosters() {
  const collector = useCollectorStore()
  const busy = ref(false)

  const hidden = computed(() => new Set(collector.mirrorPreference.hidden_hosters.map(hoster => hoster.toLowerCase())))

  const hosters = computed<HosterCount[]>(() => {
    const counts = new Map<string, number>()
    for (const candidate of collector.candidates) {
      const hoster = hosterOf(candidate)
      if (hoster) counts.set(hoster, (counts.get(hoster) ?? 0) + 1)
    }
    return [...counts]
      .map(([hoster, count]) => ({ hoster, count, hidden: hidden.value.has(hoster.toLowerCase()) }))
      .sort((a, b) => a.hoster.localeCompare(b.hoster))
  })

  /** The links the hidden hosters take off the screen, over the whole list. */
  const hiddenLinks = computed<LinkCandidate[]>(() => {
    if (!hidden.value.size) return []
    // A group key is unique inside its package only, so the rows are built per package.
    const packages = new Map<string, LinkCandidate[]>()
    for (const candidate of collector.candidates) {
      const key = candidate.package_id ?? ''
      const bucket = packages.get(key)
      if (bucket) bucket.push(candidate)
      else packages.set(key, [candidate])
    }
    const result: LinkCandidate[] = []
    for (const bucket of packages.values()) {
      const rows = mirrorRows(bucket)
      const kept = new Set(hideHosters(rows, hidden.value))
      for (const row of rows) {
        if (kept.has(row)) continue
        if (row.kind === 'link') result.push(row.candidate)
        else result.push(...row.group.members)
      }
    }
    return result
  })
  const hiddenHosterCount = computed(() => new Set(hiddenLinks.value.map(hosterOf)).size)

  async function store(next: string[]): Promise<void> {
    busy.value = true
    await collector.setMirrorPreference({ ...collector.mirrorPreference, hidden_hosters: next })
    busy.value = false
  }

  /** Hides one hoster, or shows it again. */
  async function setHidden(hoster: string, hide: boolean): Promise<void> {
    const wanted = hoster.toLowerCase()
    if (!wanted || hidden.value.has(wanted) === hide) return
    const current = collector.mirrorPreference.hidden_hosters
    await store(hide ? [...current, wanted] : current.filter(entry => entry.toLowerCase() !== wanted))
  }

  /** "Show all": every hoster comes back, including hidden ones that have no link right now. */
  async function showAll(): Promise<void> {
    if (!hidden.value.size) return
    await store([])
  }

  return { busy, hidden, hosters, hiddenLinks, hiddenHosterCount, setHidden, showAll }
}
