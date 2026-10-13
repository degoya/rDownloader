import { ref } from 'vue'

import type { DirectAddPayload } from '@/components/DirectAddForm.vue'
import { useTransfersStore } from '@/stores/transfers'

/** The direct job's dialog of the downloads view, and the refusal it keeps until the next attempt (RD-1220-03). */
export function useDirectAdd() {
  const transfers = useTransfersStore()
  const open = ref(false)
  const busy = ref(false)
  const error = ref<string | null>(null)

  function show(): void {
    error.value = null
    open.value = true
  }

  /** A queued link closes the dialog; a refusal stays in it, not in the alert behind it. */
  async function add(payload: DirectAddPayload): Promise<void> {
    busy.value = true
    error.value = null
    const ok = await transfers.add(payload.url, undefined, undefined, {
      ...(payload.categoryId ? { categoryId: payload.categoryId } : {}),
      ...(payload.accountId ? { accountId: payload.accountId } : {}),
      ...(payload.proxyProfileId ? { proxyProfileId: payload.proxyProfileId } : {}),
      priority: payload.priority
    })
    busy.value = false
    if (ok) {
      open.value = false
      return
    }
    error.value = transfers.error
    transfers.error = null
  }

  return { open, busy, error, show, add }
}
