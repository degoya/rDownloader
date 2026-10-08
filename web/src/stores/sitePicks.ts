import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { CollectorPick } from '@/api/types'
import { sameEntries } from '@/utils/sitePicks'

/** How often the board is asked again while a page resolves its entries. */
const POLL_MS = 1500
/** What the service answers for a list it no longer holds (RD-1190-17). */
const PICK_NOT_FOUND = 'site_rules.pick_not_found'

/** A page some intake listed, as the event stream announces it (RD-1190-17). */
export interface PickListing {
  list: string
  entries: number
  rule: string
}

/** Whether a failed answer says the list is gone. */
function vanished(response: { error?: unknown }): boolean {
  return (response.error as { code?: string } | undefined)?.code === PICK_NOT_FOUND
}

/**
 * The series pages whose releases wait for a choice (RD-1170-03).
 *
 * A two-stage site rule lists a page's releases and resolves none; the person picks here and the
 * service resolves the picked ones one after the other, each with its own captcha. The board
 * lives in the service's memory, so this store only mirrors it: asked when the LinkGrabber opens,
 * after a paste that listed a page, and every 1.5 s while a page is resolving, which is what the
 * "3 of 8" and "waiting for captcha" read from.
 *
 * A list can vanish under the drawer — discarded elsewhere, pushed off a full board, or gone with
 * a restart of the service (RD-1190-17). *Fetch* then lists the page again by itself, which
 * costs no captcha, and fetches the same releases from the fresh list; *Stop* and *Discard* of a
 * vanished list simply let it go. A page listed by another intake — a copied link, the browser
 * extension, Click'n'Load — is announced on the event stream and opens the drawer as a paste does.
 */
export const useSitePicksStore = defineStore('sitePicks', () => {
  const pages = ref<CollectorPick[]>([])
  const error = ref<string | null>(null)
  /** Page ids with a request of their own in flight. */
  const busy = ref<Set<string>>(new Set())
  /**
   * Raised by a paste that listed a page: the paste asked for the choice, so the drawer opens on
   * it, as "Check all" opens the indexer drawer. Nothing else opens it (`design.md`).
   */
  const asked = ref(0)
  /** Panels showing the board right now; an announcement opens the drawer where one is shown. */
  let attached = 0
  /** A page was announced while no panel was shown: the next panel opens on it. */
  const unseen = ref(false)
  /** The last announcement no panel could show, for the toast that points to the LinkGrabber. */
  const notice = ref<PickListing & { at: number } | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null

  const running = computed(() => pages.value.some(page => page.running))

  /** Asks again while a page resolves, and stops asking once none does. */
  function schedule(): void {
    if (timer) clearTimeout(timer)
    timer = null
    if (running.value) timer = setTimeout(() => { void refresh() }, POLL_MS)
  }

  async function refresh(): Promise<void> {
    const response = await api.GET('/api/v1/collector/picks')
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    error.value = null
    pages.value = response.data.pages
    schedule()
  }

  /** A paste listed a page: fetch the board and ask for the drawer. */
  async function listed(): Promise<void> {
    await refresh()
    asked.value += 1
  }

  /**
   * Another intake listed a page (RD-1190-17): fetch the board, and open the drawer where a panel
   * is shown — or remember it for the next panel and raise the notice.
   */
  async function announced(listing: PickListing): Promise<void> {
    await refresh()
    if (attached > 0) {
      asked.value += 1
      return
    }
    unseen.value = true
    notice.value = { ...listing, at: Date.now() }
  }

  /** A panel appears; answers whether a page waits that it should open on. */
  function attach(): boolean {
    attached += 1
    const opening = unseen.value
    unseen.value = false
    return opening
  }

  function detach(): void {
    attached = Math.max(0, attached - 1)
  }

  /** Puts one page's new state in place of the old one, under `id` — its own, or the vanished one's. */
  function replace(page: CollectorPick, id = page.id): void {
    const next = pages.value.filter(current => current.id !== page.id || current.id === id)
    const position = next.findIndex(current => current.id === id)
    if (position < 0) next.push(page)
    else next.splice(position, 1, page)
    pages.value = next
    schedule()
  }

  /** Drops a page the service no longer holds. */
  function forget(id: string): void {
    pages.value = pages.value.filter(page => page.id !== id)
    error.value = null
    schedule()
  }

  /**
   * Lists a vanished page again and resolves the releases chosen in it from the fresh list. The
   * first stage asks no captcha, so this costs what the paste cost.
   */
  async function relist(id: string, entries: number[]): Promise<boolean> {
    const before = pages.value.find(page => page.id === id)
    if (!before) return false
    const listed = await api.POST('/api/v1/collector/picks', { body: { address: before.address } })
    if (!listed.data) {
      error.value = responseError(listed)
      return false
    }
    const fresh = listed.data
    replace(fresh, id)
    const again = sameEntries(before.entries, fresh.entries, entries)
    if (!again.length) {
      error.value = null
      return false
    }
    const response = await api.POST('/api/v1/collector/picks/{id}/resolve', { params: { path: { id: fresh.id } }, body: { entries: again } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    replace(response.data)
    return true
  }

  async function withBusy<T>(id: string, work: () => Promise<T>): Promise<T> {
    busy.value = new Set([...busy.value, id])
    try {
      return await work()
    } finally {
      const next = new Set(busy.value)
      next.delete(id)
      busy.value = next
    }
  }

  /** Starts resolving the picked entries; answers whether the service took them. */
  async function resolve(id: string, entries: number[]): Promise<boolean> {
    if (!entries.length) return false
    return withBusy(id, async () => {
      const response = await api.POST('/api/v1/collector/picks/{id}/resolve', { params: { path: { id } }, body: { entries } })
      if (!response.data && vanished(response)) return relist(id, entries)
      if (!response.data) {
        error.value = responseError(response)
        return false
      }
      error.value = null
      replace(response.data)
      return true
    })
  }

  /** Stops a page's round: what is queued and the entry waiting for its captcha go back to pending. */
  async function cancel(id: string): Promise<void> {
    await withBusy(id, async () => {
      const response = await api.POST('/api/v1/collector/picks/{id}/cancel', { params: { path: { id } } })
      // Nothing left to stop: the list is gone, and so is the reason to say anything.
      if (!response.data && vanished(response)) return forget(id)
      if (!response.data) {
        error.value = responseError(response)
        return
      }
      error.value = null
      replace(response.data)
    })
  }

  /** Discards a page; what was resolved stays in the LinkGrabber. */
  async function discard(id: string): Promise<void> {
    await withBusy(id, async () => {
      const response = await api.DELETE('/api/v1/collector/picks/{id}', { params: { path: { id } } })
      if (!response.data && vanished(response)) return forget(id)
      if (!response.data) {
        error.value = responseError(response)
        return
      }
      error.value = null
      pages.value = pages.value.filter(page => page.id !== id)
      schedule()
    })
  }

  /** Stops asking, for the view that goes away. */
  function stop(): void {
    if (timer) clearTimeout(timer)
    timer = null
  }

  return {
    pages, error, busy, asked, unseen, notice, running,
    refresh, listed, announced, attach, detach, resolve, cancel, discard, stop
  }
})
