import { computed, onUnmounted, watch } from 'vue'

import { useCaptchasStore } from '@/stores/captchas'

/** How often the service is asked whether a browser extension checked in, in milliseconds. */
export const EXTENSION_POLL_MS = 3000

/**
 * Whether a browser extension has reported in to the service (RD-150-17).
 *
 * Only the service can know — the extension polls it — so this asks
 * `GET /api/v1/captcha-answerers` while `active` holds: at once, then every
 * `EXTENSION_POLL_MS`. A pairing done in another tab or in the extension's options then shows
 * up here without a reload, and a hint that waits for the extension goes away on its own.
 * `active` is handed the current answer, so a caller that only waits for the extension stops
 * asking once it is there.
 */
export function useExtensionConnection(active: (connected: boolean) => boolean = () => true) {
  const captchas = useCaptchasStore()
  const connected = computed(() => captchas.answerers?.browser_extension_connected === true)
  let timer: number | null = null

  function start(): void {
    if (timer !== null) return
    void captchas.refreshAnswerers()
    timer = window.setInterval(() => void captchas.refreshAnswerers(), EXTENSION_POLL_MS)
  }

  function stop(): void {
    if (timer === null) return
    window.clearInterval(timer)
    timer = null
  }

  watch(() => active(connected.value), on => (on ? start() : stop()), { immediate: true })
  onUnmounted(stop)

  return { connected }
}
