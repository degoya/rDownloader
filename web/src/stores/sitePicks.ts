import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { CollectorPick } from '@/api/types'

/** How often the board is asked again while a page resolves its entries. */
const POLL_MS = 1500

/**
 * The series pages whose releases wait for a choice (RD-1170-03).
 *
 * A two-stage site rule lists a page's releases and resolves none; the person picks here and the
 * service resolves the picked ones one after the other, each with its own captcha. The board
 * lives in the service's memory, so this store only mirrors it: asked when the LinkGrabber opens,
 * after a paste that listed a page, and every 1.5 s while a page is resolving, which is what the
 * "3 of 8" and "waiting for captcha" read from.
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

  /** Puts one page's new state in place of the old one. */
  function replace(page: CollectorPick): void {
    pages.value = pages.value.map(current => current.id === page.id ? page : current)
    schedule()
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

  return { pages, error, busy, asked, running, refresh, listed, resolve, cancel, discard, stop }
})
