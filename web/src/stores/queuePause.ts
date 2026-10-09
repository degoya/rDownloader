import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import type { AccountTrafficHold, QueuePause, QueueStopMark } from '@/api/types'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'

/** The durations the pause menu offers, in minutes (RD-190-20). */
export const PAUSE_DURATIONS = [30, 60, 180] as const

/**
 * The next time the clock shows `hh:mm`: today, or tomorrow once today's has passed. What a
 * person means by "pause until 18:00".
 */
export function nextOccurrence(clock: string, now: Date = new Date()): Date | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(clock)
  if (!match) return null
  const [hours, minutes] = [Number(match[1]), Number(match[2])]
  if (hours > 23 || minutes > 59) return null
  const at = new Date(now)
  at.setHours(hours, minutes, 0, 0)
  if (at.getTime() <= now.getTime()) at.setDate(at.getDate() + 1)
  return at
}

/**
 * The timed pause of the whole queue (RD-190-20): until when, and setting or ending it.
 *
 * The server owns the pause — its end survives a restart and resumes the queue by itself — so
 * this only mirrors it. The pause changes file states as it starts and ends, so `download.state`
 * events are what tell another tab about it; a clock ticking every second keeps the countdown
 * current and notices an end that passed without one.
 *
 * The same answer names the accounts whose traffic is used up (RD-1190-14), the other thing that
 * holds downloads back without anybody pausing them; a file running into the limit is a
 * `download.state` event too.
 *
 * And it names the stop mark (RD-1210-02): the file or package after which the queue pauses. That
 * pause has no end — `until` is `null` while `paused` holds — and only a resume ends it. Setting,
 * clearing and reaching the mark are `queue.stop_mark` events.
 */
export const useQueuePauseStore = defineStore('queuePause', () => {
  const until = ref<string | null>(null)
  /** A pause is in force; with `until` at `null` it is the stop mark's, which lasts until resumed. */
  const paused = ref(false)
  const files = ref(0)
  /** Where the queue pauses next: the stop mark in force, if any. */
  const stopMark = ref<QueueStopMark | null>(null)
  const accountTraffic = ref<AccountTrafficHold[]>([])
  const busy = ref(false)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const now = ref(Date.now())
  let clock: number | null = null

  const active = computed(() => paused.value && (until.value === null || Date.parse(until.value) > now.value))
  /** Paused at the stop mark: no end, only a resume lets the queue go on. */
  const openEnded = computed(() => paused.value && until.value === null)
  /** Seconds until the queue runs again; `0` while it is not paused. */
  const remainingSeconds = computed(() =>
    active.value && until.value ? Math.max(0, Math.round((Date.parse(until.value) - now.value) / 1000)) : 0)

  function apply(data: QueuePause): void {
    paused.value = data.paused
    until.value = data.paused ? data.until ?? null : null
    files.value = data.files
    accountTraffic.value = data.account_traffic ?? []
    stopMark.value = data.stop_mark ?? null
  }

  async function load(): Promise<void> {
    try {
      const response = await api.GET('/api/v1/queue/pause')
      if (response.data) apply(response.data)
    } catch {
      // Shown again on the next event or tick; a failed read changes nothing on the server.
    }
  }

  async function pause(body: { minutes: number } | { until: string }): Promise<boolean> {
    if (busy.value) return false
    busy.value = true
    error.value = null
    try {
      const response = await api.PUT('/api/v1/queue/pause', { body })
      if (response.data) {
        apply(response.data)
        return true
      }
      error.value = responseError(response)
      return false
    } finally {
      busy.value = false
    }
  }

  function pauseFor(minutes: number): Promise<boolean> {
    return pause({ minutes })
  }

  function pauseUntil(at: Date): Promise<boolean> {
    return pause({ until: at.toISOString() })
  }

  /** Ends the pause now; answers how many files it queued again, `null` when refused. */
  async function resume(): Promise<number | null> {
    if (busy.value) return null
    busy.value = true
    error.value = null
    try {
      const response = await api.DELETE('/api/v1/queue/pause')
      if (response.data) {
        paused.value = false
        until.value = null
        files.value = 0
        // A start by hand lets go of the accounts held for their traffic as well.
        accountTraffic.value = []
        return response.data.resumed
      }
      error.value = responseError(response)
      return null
    } finally {
      busy.value = false
    }
  }

  /** Puts the stop mark on a file or a package, replacing the one in force; `false` when refused. */
  async function setStopMark(target: { download_id: string } | { package_id: string }): Promise<boolean> {
    error.value = null
    const response = await api.PUT('/api/v1/queue/stop-mark', { body: target })
    if (response.data) {
      stopMark.value = response.data
      return true
    }
    error.value = responseError(response)
    return false
  }

  /** Removes the stop mark; `false` when refused. */
  async function clearStopMark(): Promise<boolean> {
    error.value = null
    const response = await api.DELETE('/api/v1/queue/stop-mark')
    if (response.data) {
      stopMark.value = null
      return true
    }
    error.value = responseError(response)
    return false
  }

  /** Whether the stop mark sits on this file or package. */
  function marks(kind: 'download' | 'package', id: string): boolean {
    const mark = stopMark.value
    if (!mark) return false
    return kind === 'download' ? mark.download_id === id : mark.package_id === id
  }

  const events = debouncedEventRefresh(['download.state', 'queue.stop_mark'], load, { delayMs: 400 })

  function tick(): void {
    const wasActive = active.value
    now.value = Date.now()
    // The server ends the pause on its own tick; read back what it made of it.
    if (wasActive && !active.value) events.schedule()
  }

  /** Connected app-wide by `App.vue` once there is a session, like the transfer list. */
  function connect(): void {
    if (clock !== null) return
    events.connect()
    clock = window.setInterval(tick, 1_000)
    void load()
  }

  function disconnect(): void {
    events.disconnect()
    if (clock !== null) window.clearInterval(clock)
    clock = null
  }

  return {
    until, paused, files, accountTraffic, stopMark, busy, error, active, openEnded, remainingSeconds,
    load, pauseFor, pauseUntil, resume, setStopMark, clearStopMark, marks, connect, disconnect
  }
})
