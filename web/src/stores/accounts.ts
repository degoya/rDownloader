import { defineStore, storeToRefs } from 'pinia'

import { api } from '@/api/client'
import type { Account } from '@/api/types'

import { sharedRead } from './sharedRead'

/** The provider accounts, read once for every view that lists or picks one (WEB-3). */
export const useAccountsStore = defineStore('accounts', () => {
  const shared = sharedRead(() => api.GET('/api/v1/accounts'), [] as Account[], ['account.changed'])
  return { accounts: shared.value, fetchAccounts: shared.load, follow: shared.follow }
})

/**
 * The accounts for a component: the shared list, kept current while the component lives, and
 * `fetchAccounts()` for the read it makes when it opens.
 */
export function useAccounts() {
  const store = useAccountsStore()
  store.follow()
  return { accounts: storeToRefs(store).accounts, fetchAccounts: store.fetchAccounts }
}
