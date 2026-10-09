import { useToast } from '@nuxt/ui/composables'
import { watch } from 'vue'

import { useTransfersStore } from '@/stores/transfers'

/**
 * The download list's notices as toasts rather than a card above the list (RD-1220-03).
 *
 * `transfers.notice` is what went through — it goes by itself; `transfers.warning` is what was
 * left untouched or refused — it stays until it is closed, since the reader may need the names in
 * it. Each is taken from the store once shown, so the same sentence twice is two toasts, and a
 * notice set on another page (`p`, the pause menu) is shown on arrival, as the card was. The id is
 * the sentence: a repeat while the first is still open pulses that one instead of stacking.
 */
export function useQueueNoticeToasts(): void {
  const toast = useToast()
  const transfers = useTransfersStore()

  watch(() => transfers.notice, (text) => {
    if (!text) return
    toast.add({ id: `queue-notice:${text}`, title: text, color: 'info', icon: 'i-lucide-info' })
    transfers.notice = null
  }, { immediate: true })

  watch(() => transfers.warning, (text) => {
    if (!text) return
    toast.add({ id: `queue-warning:${text}`, title: text, color: 'warning', icon: 'i-lucide-triangle-alert', duration: 0 })
    transfers.warning = null
  }, { immediate: true })
}
