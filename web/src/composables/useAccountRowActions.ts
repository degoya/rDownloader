import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import type { Account, UpdateAccount } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'

/**
 * The switch and the delete button of an account's row in the accounts card: each writes at
 * once, says so in the card's notices, and changes the row in the shared list.
 */
export function useAccountRowActions(card: {
  accounts: Ref<Account[]>
  error: Ref<string | null>
  message: Ref<string | null>
  editingAccountId: Ref<string | null>
  clearTestResult: (id: string) => void
  resetAccountForm: () => void
}) {
  const { t } = useI18n()
  const confirm = useConfirm()
  const { accounts, error, message, editingAccountId, clearTestResult, resetAccountForm } = card
  const togglingAccountId = ref<string | null>(null)
  const deletingAccountId = ref<string | null>(null)

  // Switches one account on or off from the list.
  //
  // Uses the ordinary update endpoint rather than a pair of routes of its own: `update_account`
  // already leaves the vault references untouched when no secret is sent, and a second write path
  // would be one more place that could get that wrong.
  async function setAccountEnabled(account: Account, enabled: boolean): Promise<void> {
    togglingAccountId.value = account.id
    error.value = null
    message.value = null
    const body: UpdateAccount = {
      provider: account.provider,
      label: account.label,
      username: account.username ?? null,
      credential_mode: account.credential_mode ?? null,
      proxy_profile_id: account.proxy_profile_id ?? null,
      enabled,
      // The stored secret and cookies stay exactly as they are: nothing is sent, nothing cleared.
      clear_secret: false,
      clear_cookies: false
    }
    const response = await api.PUT('/api/v1/accounts/{id}', {
      params: { path: { id: account.id } },
      body
    })
    togglingAccountId.value = null
    if (!response.data) {
      // The switch is bound to the account, so a failed write leaves it where it was.
      error.value = responseError(response)
      return
    }
    accounts.value = accounts.value.map(entry => entry.id === response.data!.id ? response.data! : entry)
    message.value = t('network.messages.updated')
  }

  async function deleteAccount(account: Account): Promise<void> {
    const confirmed = await confirm({
      title: t('network.delete.title'),
      description: t('network.delete.description', { label: account.label }),
      confirmLabel: t('network.delete.confirm'),
      confirmIcon: 'i-lucide-user-x',
      destructive: true
    })
    if (!confirmed) return
    deletingAccountId.value = account.id
    error.value = null
    message.value = null
    const response = await api.DELETE('/api/v1/accounts/{id}', {
      params: { path: { id: account.id } }
    })
    deletingAccountId.value = null
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    accounts.value = accounts.value.filter(item => item.id !== account.id)
    clearTestResult(account.id)
    if (editingAccountId.value === account.id) resetAccountForm()
    message.value = resultMessage(response.data)
  }

  return { togglingAccountId, deletingAccountId, setAccountEnabled, deleteAccount }
}
