import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { StreamChannel, StreamSchedule, StreamScheduleRequest, StreamScheduledRun } from '@/api/types'

/** Shared so the nav badge shows the channel count without opening the Streams route. */
export const useStreamsStore = defineStore('streams', () => {
  const channels = ref<StreamChannel[]>([])
  const schedules = ref<StreamSchedule[]>([])
  const runs = ref<StreamScheduledRun[]>([])
  const error = ref<string | null>(null)

  async function refresh(): Promise<void> {
    const response = await api.GET('/api/v1/streams/channels')
    if (response.data) {
      channels.value = response.data
      error.value = null
    } else {
      error.value = responseError(response)
    }
  }

  /** Schedules and their occurrences (RD-080-08). */
  async function refreshSchedules(): Promise<void> {
    const [scheduleResponse, runResponse] = await Promise.all([
      api.GET('/api/v1/streams/schedules'),
      api.GET('/api/v1/streams/runs')
    ])
    if (scheduleResponse.data) schedules.value = scheduleResponse.data
    else error.value = responseError(scheduleResponse)
    if (runResponse.data) runs.value = runResponse.data
  }

  async function saveSchedule(body: StreamScheduleRequest, id?: string): Promise<boolean> {
    const response = id
      ? await api.PUT('/api/v1/streams/schedules/{id}', { params: { path: { id } }, body })
      : await api.POST('/api/v1/streams/schedules', { body })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refreshSchedules()
    return true
  }

  async function removeSchedule(id: string): Promise<void> {
    const response = await api.DELETE('/api/v1/streams/schedules/{id}', { params: { path: { id } } })
    if (response.error) {
      error.value = responseError(response)
      return
    }
    await refreshSchedules()
  }

  return {
    channels,
    schedules,
    runs,
    error,
    refresh,
    refreshSchedules,
    saveSchedule,
    removeSchedule
  }
})
