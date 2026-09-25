import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { PostprocessPluginStep, PostprocessQueueEntry, PostprocessStage } from '@/api/types'

export const usePostprocessStore = defineStore('postprocess', () => {
  const queue = ref<PostprocessQueueEntry[]>([])
  const scripts = ref<string[]>([])
  const scriptsDirectory = ref<string | null>(null)
  const pluginSteps = ref<PostprocessPluginStep[]>([])
  const error = ref<string | null>(null)
  let scriptsLoaded = false
  let pluginStepsLoaded = false
  let refreshing: Promise<void> | null = null

  const active = computed(() => queue.value.length > 0)

  /** Reloads the pipeline queue; concurrent calls share one request. */
  function refresh(): Promise<void> {
    if (refreshing) return refreshing
    refreshing = (async () => {
      const response = await api.GET('/api/v1/postprocess/queue')
      if (response.data) {
        queue.value = response.data
        error.value = null
      } else {
        error.value = responseError(response)
      }
    })().finally(() => { refreshing = null })
    return refreshing
  }

  /** Loads the script names once (or again when `force` is set). */
  async function loadScripts(force = false): Promise<string[]> {
    if (scriptsLoaded && !force) return scripts.value
    const response = await api.GET('/api/v1/postprocess/scripts')
    if (response.data) {
      scripts.value = response.data.scripts
      scriptsDirectory.value = response.data.directory
      scriptsLoaded = true
    } else {
      error.value = responseError(response)
    }
    return scripts.value
  }

  /**
   * Loads the installed post-processing step plugins once.
   *
   * They are compiled at startup and a newly installed one needs a restart anyway, so there
   * is nothing to gain from asking again while the page is open.
   */
  async function loadPluginSteps(force = false): Promise<PostprocessPluginStep[]> {
    if (pluginStepsLoaded && !force) return pluginSteps.value
    const response = await api.GET('/api/v1/postprocess/plugin-steps')
    if (response.data) {
      pluginSteps.value = response.data
      pluginStepsLoaded = true
    }
    return pluginSteps.value
  }

  /** Applies one `postprocess.progress` event to the matching queue entry. */
  function applyProgress(packageId: string, stage: PostprocessStage | null, percent: number | null, current: string | null): void {
    const entry = queue.value.find(item => item.package_id === packageId)
    if (!entry) return
    entry.stage = stage ?? entry.stage ?? null
    entry.percent = percent
    entry.current = current
    entry.pending = false
  }

  return {
    queue,
    scripts,
    scriptsDirectory,
    pluginSteps,
    error,
    active,
    refresh,
    loadScripts,
    loadPluginSteps,
    applyProgress
  }
})
