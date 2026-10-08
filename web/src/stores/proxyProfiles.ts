import { defineStore, storeToRefs } from 'pinia'

import { api } from '@/api/client'
import type { ProxyProfile } from '@/api/types'

import { sharedRead } from './sharedRead'

/** The proxy profiles, read once for every view that lists or picks one (WEB-3). */
export const useProxyProfilesStore = defineStore('proxyProfiles', () => {
  const shared = sharedRead(() => api.GET('/api/v1/proxy-profiles'), [] as ProxyProfile[], ['proxy.changed'])
  return { proxies: shared.value, fetchProxies: shared.load, follow: shared.follow }
})

/**
 * The proxy profiles for a component: the shared list, kept current while the component lives,
 * and `fetchProxies()` for the read it makes when it opens.
 */
export function useProxyProfiles() {
  const store = useProxyProfilesStore()
  store.follow()
  return { proxies: storeToRefs(store).proxies, fetchProxies: store.fetchProxies }
}
