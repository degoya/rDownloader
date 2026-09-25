import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'
import type {
  Automation,
  AutomationDryRun,
  AutomationRequest,
  AutomationRun,
  AutomationVocabulary
} from '@/api/types'

/**
 * Automations, their run history and the vocabulary the editor builds its forms from.
 *
 * The vocabulary comes from the server rather than being written out here twice: a trigger
 * or operator the editor offers but the API refuses would be a form that cannot be saved,
 * and that is exactly the kind of drift a second hand-kept list produces.
 */
export const useAutomationsStore = defineStore('automations', () => {
  const automations = ref<Automation[]>([])
  const runs = ref<AutomationRun[]>([])
  const vocabulary = ref<AutomationVocabulary | null>(null)
  const error = ref<string | null>(null)
  const busy = ref(false)
  /** True while the automation list is being fetched — `busy` covers the write actions. */
  const fetching = ref(false)
  /** True once the first fetch has settled, so "no automations" is only said when it is true. */
  const settled = ref(false)
  /**
   * What the view shows in place of an empty list while the **first** fetch is on its way
   * (RD-104-07).
   *
   * Not `fetching.value || !settled.value` (RD-106-19): `fetching` goes true on every
   * `refresh()`, and every save, delete and toggle ends in one, so an empty list traded its
   * empty state for the loading skeleton and back on each of them — the reported flicker.
   * `design.md` promises that surface for the first fetch alone. `settled` is set even when
   * that first fetch failed, which is what we want: `loading` turns false and `DataState`
   * renders the error it prefers over the empty state. `fetching` itself is untouched as the
   * per-refresh flag, and `busy` still covers the write actions the buttons bind.
   */
  const loading = computed(() => !settled.value)

  let releaseEvents: (() => void) | null = null
  let refreshTimer: number | null = null

  async function refresh(): Promise<void> {
    fetching.value = true
    const [list, history] = await Promise.all([
      api.GET('/api/v1/automations'),
      api.GET('/api/v1/automations/runs')
    ])
    if (list.data) {
      automations.value = list.data
      error.value = null
    } else {
      error.value = responseError(list)
    }
    if (history.data) runs.value = history.data
    fetching.value = false
    settled.value = true
  }

  /**
   * Loads the trigger/condition/action catalogue that drives every dropdown in the editor.
   *
   * A failure used to be swallowed: `vocabulary` stayed null, each `?? []` fallback produced an
   * empty list, and the view showed four empty dropdowns with no error at all. It now reports
   * the failure like every other action here, and a later call retries instead of being turned
   * away by the "already loaded" guard.
   */
  async function loadVocabulary(): Promise<void> {
    if (vocabulary.value) return
    const response = await api.GET('/api/v1/automations/vocabulary')
    if (response.data) {
      vocabulary.value = response.data
      error.value = null
    } else {
      error.value = responseError(response)
    }
  }

  async function save(body: AutomationRequest, id?: string): Promise<boolean> {
    busy.value = true
    const response = id
      ? await api.PUT('/api/v1/automations/{id}', { params: { path: { id } }, body })
      : await api.POST('/api/v1/automations', { body })
    busy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  async function setEnabled(id: string, enabled: boolean): Promise<void> {
    const response = await api.POST('/api/v1/automations/{id}/enable', {
      params: { path: { id } },
      body: { enabled }
    })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  async function remove(id: string): Promise<void> {
    const response = await api.DELETE('/api/v1/automations/{id}', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  /** Evaluates a trigger and a sample package. Never has an effect. */
  async function dryRun(trigger: string, packageId: string | null): Promise<AutomationDryRun[]> {
    const response = await api.POST('/api/v1/automations/dry-run', {
      body: { trigger, package_id: packageId ?? undefined } as never
    })
    if (!response.data) {
      error.value = responseError(response)
      return []
    }
    error.value = null
    return response.data
  }

  /** Re-reads the list and the run history, at most once per burst of events. */
  function scheduleRefresh(): void {
    if (refreshTimer !== null) return
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null
      // Re-arm instead of stacking a second request on top of one already in flight; a burst
      // of events would otherwise multiply into parallel round trips.
      if (fetching.value) return scheduleRefresh()
      void refresh()
    }, 300)
  }

  /**
   * What the bus says about automations.
   *
   * This store subscribed to nothing until now, because the server published nothing: an
   * automation created, enabled, disabled or deleted in a second tab — or by an import — was
   * invisible here until the page was reloaded, and the toggle a user then flipped was
   * flipped against a list from minutes ago.
   *
   * Re-read rather than patched. The payload names the automation that changed, but this store
   * holds two lists from two endpoints: `refresh()` reads the automations *and* the run
   * history, and a run the change produced cannot be reconstructed from an id. Every write
   * here already ends in the same `refresh()`, so the event path is the one the store is
   * built around rather than a second way of maintaining the same state. Debounced because a
   * bulk import emits one event per automation. No notice is raised — `design.md` has no
   * pattern for announcing that data caught up.
   */
  function connectEvents(): void {
    if (releaseEvents) return
    releaseEvents = subscribeEvents({ 'automation.changed': scheduleRefresh })
  }

  function disconnectEvents(): void {
    releaseEvents?.()
    releaseEvents = null
    if (refreshTimer !== null) {
      window.clearTimeout(refreshTimer)
      refreshTimer = null
    }
  }

  return {
    automations,
    runs,
    vocabulary,
    error,
    busy,
    loading,
    refresh,
    loadVocabulary,
    save,
    setEnabled,
    remove,
    dryRun,
    connectEvents,
    disconnectEvents
  }
})
