import type { Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import type { ConfirmOptions } from '@/composables/useConfirm'

type MfaCredential = components['schemas']['MfaCredential']

/**
 * Removing a second factor, an authenticator app's or a passkey (RD-1280-11): the card's own
 * question, then the password the card asked for again, then the list fetched anew. The two
 * cards differ only in the question.
 */
export function useMfaCredentials(card: {
  confirm: (options: ConfirmOptions) => Promise<boolean>
  password: Ref<string>
  error: Ref<string | null>
  reload: () => Promise<void>
}): { remove: (credential: MfaCredential, question: ConfirmOptions) => Promise<void> } {
  async function remove(credential: MfaCredential, question: ConfirmOptions): Promise<void> {
    if (!(await card.confirm(question))) return
    const response = await api.DELETE('/api/v1/mfa/credentials/{id}', {
      params: { path: { id: credential.id } },
      body: { password: card.password.value }
    })
    if (!response.data) {
      card.error.value = responseError(response)
      return
    }
    card.password.value = ''
    await card.reload()
  }
  return { remove }
}
