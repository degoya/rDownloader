import { ref } from 'vue'

import { api } from '@/api/client'

/**
 * The hosters each account covers, read when its list is first opened, and the one filter all
 * the open lists share.
 */
export function useAccountHosters() {
  const hostersByAccount = ref<Record<string, string[]>>({})
  const hosterFilter = ref('')
  const hostersLoadingId = ref<string | null>(null)

  function visibleHosters(accountId: string): string[] {
    const hosters = hostersByAccount.value[accountId] ?? []
    const needle = hosterFilter.value.trim().toLowerCase()
    return needle ? hosters.filter(host => host.includes(needle)) : hosters
  }

  async function loadHosters(accountId: string): Promise<void> {
    if (hostersByAccount.value[accountId]) return
    hostersLoadingId.value = accountId
    const response = await api.GET('/api/v1/accounts/{id}/hosters', { params: { path: { id: accountId } } })
    hostersLoadingId.value = null
    if (response.data) hostersByAccount.value = { ...hostersByAccount.value, [accountId]: response.data.hosters }
  }

  return { hostersByAccount, hosterFilter, hostersLoadingId, visibleHosters, loadHosters }
}
