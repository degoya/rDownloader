import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Account, AccountTest } from '@/api/types'
import { translateAccountLabel } from '@/i18n/server'
import { formatBytes } from '@/utils/format'

/**
 * The account checks of the accounts tab: which one runs, and the last result per account.
 *
 * A check reports into the tab's own `error` and `message` notices.
 */
export function useAccountTests(error: Ref<string | null>, message: Ref<string | null>) {
  const { t } = useI18n()
  /** Last successful test per account id, so the outcome survives the transient alert. */
  const testResults = ref<Record<string, AccountTest>>({})
  const testingAccountId = ref<string | null>(null)

  function clearTestResult(accountId: string): void {
    const { [accountId]: _removed, ...rest } = testResults.value
    testResults.value = rest
  }

  /** Remaining traffic when the provider reports it, otherwise the translated label parts. */
  function testBadge(accountId: string): string {
    const result = testResults.value[accountId]
    if (!result) return ''
    return result.traffic_left
      ? t('network.messages.traffic_left', { amount: formatBytes(result.traffic_left) })
      : translateAccountLabel(result.label)
  }

  /// Checks an account right after it was saved, without making the dialog wait for the answer.
  ///
  /// Deliberately not awaited. A check reaches all the way into the provider's resolver, and that
  /// can take a while — DDownload's sign-in queues a captcha for somebody to answer, which parks
  /// it for as long as the queue allows. Blocking the form on that would be worse than the silence
  /// it replaces; the row already has a spinner and a badge for the result.
  ///
  /// A disabled account is skipped: the endpoint refuses one with `account.disabled`, and somebody
  /// who deliberately created it switched off does not need that reported back at them.
  function checkAfterSaving(account: Account): void {
    if (!account.enabled) return
    void testAccount(account, { quiet: true })
  }

  async function testAccount(account: Account, options: { quiet?: boolean } = {}): Promise<void> {
    testingAccountId.value = account.id
    if (!options.quiet) {
      error.value = null
      message.value = null
    }
    const response = await api.POST('/api/v1/accounts/{id}/test', {
      params: { path: { id: account.id } }
    })
    testingAccountId.value = null
    if (!response.data) {
      // Reported even when the check ran on its own: a saved account that does not work is the
      // one thing worth interrupting for, and it is why the check happens at save time at all.
      error.value = responseError(response)
      return
    }
    testResults.value = { ...testResults.value, [account.id]: response.data }
    if (!options.quiet) message.value = accountTestMessage(account, response.data)
  }

  function accountTestMessage(account: Account, result: AccountTest): string {
    const label = translateAccountLabel(result.label)
    const parts = [
      ...(label ? [label] : []),
      result.premium ? t('network.messages.premium_active') : t('network.messages.premium_inactive')
    ]
    if (result.traffic_left) parts.push(t('network.messages.traffic_left', { amount: formatBytes(result.traffic_left) }))
    return `${account.label}: ${parts.join(' · ')}`
  }

  return { testResults, testingAccountId, clearTestResult, testBadge, checkAfterSaving, testAccount }
}
