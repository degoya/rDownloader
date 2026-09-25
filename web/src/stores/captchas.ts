import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type { CaptchaAnswerers, PendingCaptcha } from '@/api/types'
import { subscribeEvents } from '@/composables/useEventStream'

/** Payload of the `captcha.changed` server event: the complete current list. */
interface CaptchaChangedPayload {
  pending?: PendingCaptcha[]
}

/**
 * Server events arrive as the whole envelope, with the event's own data nested under
 * `payload` — the list is at `payload.pending`, not at the top level.
 */
interface EventEnvelope {
  payload?: CaptchaChangedPayload
}

/** Result of answering or declining a captcha, with the translated server message. */
export interface CaptchaOutcome {
  ok: boolean
  message: string
}

export const useCaptchasStore = defineStore('captchas', () => {
  /** Waiting challenges, oldest first (the order the API returns them in). */
  const pending = ref<PendingCaptcha[]>([])
  const busy = ref(false)
  const error = ref<string | null>(null)
  /**
   * Who is around to answer a widget captcha, as far as the server can tell: it knows when a
   * browser extension last polled, which this page cannot know on its own (RD-108-02).
   * `null` until asked; the dialog asks while a widget challenge is shown.
   */
  const answerers = ref<CaptchaAnswerers | null>(null)
  let releaseEvents: (() => void) | null = null

  /** Only one challenge is ever presented; the rest wait their turn. */
  const current = computed<PendingCaptcha | null>(() => pending.value[0] ?? null)

  async function refresh(): Promise<void> {
    // A failed reload must not reject: this also runs from the event handler, where an
    // unhandled rejection would surface as a console error instead of store state.
    let response
    try {
      response = await api.GET('/api/v1/captchas')
    } catch (problem) {
      error.value = problem instanceof Error ? problem.message : String(problem)
      return
    }
    if (response.data) {
      pending.value = response.data
      error.value = null
    } else {
      error.value = responseError(response)
    }
  }

  /** Asks the server whether a browser extension is connected. Never rejects, like `refresh`. */
  async function refreshAnswerers(): Promise<void> {
    try {
      const response = await api.GET('/api/v1/captcha-answerers')
      if (response.data) answerers.value = response.data
    } catch {
      // The hint falls back to the neutral wording; the queue itself is unaffected.
    }
  }

  async function solve(id: string, token: string): Promise<CaptchaOutcome> {
    busy.value = true
    const response = await api.POST('/api/v1/captchas/{id}/solution', {
      params: { path: { id } },
      body: { token }
    })
    busy.value = false
    return complete(id, response)
  }

  /**
   * Answers a click-point captcha with the spot the person clicked, in pixels of the image as
   * the hoster served it (RD-110-15). The server refuses it for any other kind.
   */
  async function click(id: string, x: number, y: number): Promise<CaptchaOutcome> {
    busy.value = true
    const response = await api.POST('/api/v1/captchas/{id}/click', {
      params: { path: { id } },
      body: { x, y }
    })
    busy.value = false
    return complete(id, response)
  }

  async function skip(id: string): Promise<CaptchaOutcome> {
    busy.value = true
    const response = await api.POST('/api/v1/captchas/{id}/skip', { params: { path: { id } } })
    busy.value = false
    return complete(id, response)
  }

  /**
   * Drops the answered challenge locally so the dialog advances without waiting for the
   * event, and translates the coded server message. A challenge that is no longer waiting
   * (expired, already answered) is gone either way, so it is dropped too.
   */
  function complete(id: string, response: { data?: unknown, error?: unknown }): CaptchaOutcome {
    pending.value = pending.value.filter(item => item.id !== id)
    if (!response.data) {
      const message = responseError(response)
      error.value = message
      return { ok: false, message }
    }
    error.value = null
    return { ok: true, message: resultMessage(response.data) }
  }

  function applyChanged(event: MessageEvent<string>): void {
    let envelope: EventEnvelope
    try {
      envelope = JSON.parse(event.data) as EventEnvelope
    } catch {
      void refresh()
      return
    }
    const list = envelope.payload?.pending
    // An envelope without the expected list means the shape changed under us; ask the API
    // rather than silently showing a stale queue.
    if (Array.isArray(list)) pending.value = list
    else void refresh()
  }

  function connectEvents(): void {
    if (releaseEvents) return
    releaseEvents = subscribeEvents({ 'captcha.changed': applyChanged })
  }

  function disconnectEvents(): void {
    releaseEvents?.()
    releaseEvents = null
  }

  return {
    pending,
    current,
    busy,
    error,
    answerers,
    refresh,
    refreshAnswerers,
    solve,
    click,
    skip,
    connectEvents,
    disconnectEvents
  }
})
