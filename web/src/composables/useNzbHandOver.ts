import { useToast } from '@nuxt/ui/composables'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Account, DownloadPackage, NzbImport } from '@/api/types'
import { useAccountProviders } from '@/composables/useAccountProviders'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useNzbImportsStore, type NzbHandOverResult } from '@/stores/nzbImports'
import { showNzbHandOver, type NzbHandOverPlace } from '@/utils/nzbHandOver'

/** One account an NZB import can be handed to. */
export interface NzbHandOverTarget {
  accountId: string
  /** The account and its provider, as the menu lists it. */
  label: string
  /** The provider's display name, as the toasts and the badge name it. */
  provider: string
}

/**
 * The accounts, and the provider slugs whose remote-job plugin takes NZB files. Module-level,
 * like `useAccountProviders`: every NZB row of the LinkGrabber asks the same question, and one
 * read answers all of them. `null` until the first answer.
 */
const accounts = ref<Account[]>([])
const nzbProviders = ref<Set<string> | null>(null)
let loaded = false

async function refresh(): Promise<void> {
  const [accountsResponse, providersResponse] = await Promise.all([
    api.GET('/api/v1/accounts'),
    api.GET('/api/v1/remote-jobs/providers', { params: { query: { container: 'nzb' } } })
  ])
  accounts.value = accountsResponse.data ?? []
  nzbProviders.value = new Set((providersResponse.data ?? []).map(slug => slug.toLowerCase()))
}

/** An account added or removed, or a remote-job plugin installed or removed, changes the menu. */
const events = debouncedEventRefresh(['plugin_catalog.changed', 'account.changed'], refresh)

/**
 * Handing NZBs to a remote-job provider (RD-191-13): imports from the LinkGrabber, and the NZB
 * behind a package from the Downloads view (`place`), each behind a settings switch of its own.
 *
 * Offered only for accounts whose provider's plugin declares it takes NZB files — the server
 * answers that from the installed manifests (`GET /api/v1/remote-jobs/providers?container=nzb`),
 * so there is no list of providers here. The provider fetches the NZB from Usenet itself; what
 * it finished comes back into the LinkGrabber as links, the way every remote job's result does.
 *
 * Reported like a file import: one import gets a toast that names it, several get one counted
 * summary.
 */
export function useNzbHandOver(place: NzbHandOverPlace) {
  if (!loaded) {
    loaded = true
    void refresh()
    // Never released: the state is module-level and outlives every row that reads it.
    events.connect()
  }
  const { t } = useI18n()
  const toast = useToast()
  const nzb = useNzbImportsStore()
  const { providerName } = useAccountProviders()

  // Switched off for this place, or without an account that takes NZBs, nobody is offered:
  // every menu entry and the selection bar's entry read `targets`, while `handedOverTo` keeps
  // naming the provider of an earlier hand-over.
  const targets = computed<NzbHandOverTarget[]>(() => {
    const providers = nzbProviders.value
    if (!providers || !showNzbHandOver[place].value) return []
    return accounts.value
      .filter(account => account.enabled && providers.has(account.provider.toLowerCase()))
      .map(account => ({
        accountId: account.id,
        label: `${account.label} · ${providerName(account.provider)}`,
        provider: providerName(account.provider)
      }))
  })

  /** The provider an import went to, or `null` when it was not handed over. */
  function handedOverTo(item: NzbImport): string | null {
    const handOver = item.handed_over
    if (!handOver) return null
    const account = accounts.value.find(entry => entry.id === handOver.account_id)
    return account ? providerName(account.provider) : t('linkgrabber.nzb.hand_over.provider_fallback')
  }

  /** The provider the NZB behind a package went to, or `null`; the mark is its import's. */
  function packageHandedOverTo(item: DownloadPackage): string | null {
    const imported = item.nzb_import_id ? nzb.imports.find(entry => entry.id === item.nzb_import_id) : undefined
    return imported ? handedOverTo(imported) : null
  }

  /** Hands `ids` to one account, one request after another, and reports the outcome. */
  async function handOver(ids: string[], accountId: string): Promise<void> {
    const names = new Map(nzb.imports.map(item => [item.id, item.name]))
    await submitEach(ids.map(id => ({ id, name: names.get(id) ?? '' })), accountId, nzb.handOver)
  }

  /** Hands the NZB behind one package to one account, from the Downloads view. */
  async function handOverPackage(item: Pick<DownloadPackage, 'id' | 'name'>, accountId: string): Promise<void> {
    await submitEach([{ id: item.id, name: item.name }], accountId, nzb.handOverPackage)
  }

  async function submitEach(
    entries: { id: string, name: string }[],
    accountId: string,
    submit: (id: string, accountId: string) => Promise<NzbHandOverResult>
  ): Promise<void> {
    const provider = targets.value.find(target => target.accountId === accountId)?.provider
      ?? t('linkgrabber.nzb.hand_over.provider_fallback')
    const results: NzbHandOverResult[] = []
    // Sequential: the jobs are charged to one account, and the provider sees them in order.
    for (const entry of entries) results.push(await submit(entry.id, accountId))
    if (results.length === 1) {
      const name = entries[0]!.name
      const result = results[0]!
      if (!result.ok) {
        toast.add({ title: t('linkgrabber.nzb.hand_over.failed', { name }), description: result.message, color: 'error', icon: 'i-lucide-circle-alert' })
        return
      }
      const key = result.alreadyRunning ? 'linkgrabber.nzb.hand_over.already_running' : 'linkgrabber.nzb.hand_over.started'
      toast.add({ title: t(key, { name, provider }), color: 'success', icon: 'i-lucide-cloud-upload' })
      return
    }
    const reasons = results.flatMap(result => result.ok ? [] : [result.message])
    const summary = { done: results.length - reasons.length, total: results.length, failed: reasons.length, provider }
    if (reasons.length) {
      // The first reason stands for the rest: a batch to one account tends to fail for one cause.
      toast.add({ title: t('linkgrabber.nzb.hand_over.summary_failed', { ...summary, reason: reasons[0] }), color: 'warning', icon: 'i-lucide-circle-alert' })
    } else {
      toast.add({ title: t('linkgrabber.nzb.hand_over.summary', summary), color: 'success', icon: 'i-lucide-cloud-upload' })
    }
  }

  /** The menu of targets for the imports `ids()` names when an entry is picked. */
  function menuItems(ids: () => string[]) {
    return targets.value.map(target => ({
      label: target.label,
      icon: 'i-lucide-cloud-upload',
      onSelect: () => { void handOver(ids(), target.accountId) }
    }))
  }

  return { targets, handedOverTo, packageHandedOverTo, handOver, handOverPackage, menuItems }
}
