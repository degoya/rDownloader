import { useToast } from '@nuxt/ui/composables'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import { handOverCollectorPackage, type CandidateHandOverMark } from '@/api/packageHandOver'
import type { Account, CollectorPackage, LinkCandidate } from '@/api/types'
import { useAccountProviders } from '@/composables/useAccountProviders'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useErrorToast } from '@/composables/useErrorToast'
import { translateServerMessage } from '@/i18n/server'
import { useAccountsStore } from '@/stores/accounts'
import { useCollectorStore } from '@/stores/collector'
import { showPackageHandOver } from '@/utils/packageHandOver'

/** One account a LinkGrabber package can be handed to. */
export interface PackageHandOverTarget {
  accountId: string
  /** The account and its provider, as the menu lists it. */
  label: string
  /** The provider's display name, as the toast names it. */
  provider: string
}

/**
 * The accounts, the provider slugs an installed remote-job plugin runs jobs on, and those whose
 * plugin also takes a `.torrent` file. Module-level, like `useNzbHandOver`: every package row of
 * the LinkGrabber asks the same question, and one read answers all of them.
 */
const accounts = ref<Account[]>([])
const linkProviders = ref<Set<string> | null>(null)
const torrentProviders = ref<Set<string>>(new Set())
let loaded = false

const lowered = (slugs: string[] | undefined) => new Set((slugs ?? []).map(slug => slug.toLowerCase()))

async function refresh(): Promise<void> {
  const [accountsResponse, providersResponse, torrentResponse] = await Promise.all([
    useAccountsStore().fetchAccounts(),
    api.GET('/api/v1/remote-jobs/providers'),
    api.GET('/api/v1/remote-jobs/providers', { params: { query: { container: 'torrent' } } })
  ])
  accounts.value = accountsResponse.data ?? []
  linkProviders.value = lowered(providersResponse.data)
  torrentProviders.value = lowered(torrentResponse.data)
}

/** An account added or removed, or a remote-job plugin installed or removed, changes the menu. */
const events = debouncedEventRefresh(['plugin_catalog.changed', 'account.changed'], refresh)

/** What kinds of remote-job source a package's links make (RD-1440-07). */
function sourceKinds(candidates: readonly LinkCandidate[]): { links: boolean, torrentFiles: boolean } {
  return {
    links: candidates.some(candidate => /^(magnet:|https?:)/i.test(candidate.url)),
    torrentFiles: candidates.some(candidate => /^file:.*\.torrent$/i.test(candidate.url))
  }
}

/**
 * Handing a LinkGrabber package to a remote-job provider (RD-1440-07), from the package's menu.
 *
 * Offered only for accounts whose provider's plugin takes one of the package's kinds of source:
 * a magnet or an address any remote-job plugin may claim, a `.torrent` file only one that
 * declares the container (`GET /api/v1/remote-jobs/providers?container=torrent`). Whether the
 * links handed over leave the LinkGrabber is the service's setting; the toast counts what went
 * and what stayed, and names the first reason.
 */
export function usePackageHandOver() {
  if (!loaded) {
    loaded = true
    void refresh()
    // Never released: the state is module-level and outlives every row that reads it.
    events.connect()
  }
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()
  const collector = useCollectorStore()
  const { providerName } = useAccountProviders()
  const handingOverIds = ref<Set<string>>(new Set())

  /** The accounts a package with these links can go to; none while the switch is off. */
  function targets(candidates: readonly LinkCandidate[]): PackageHandOverTarget[] {
    const providers = linkProviders.value
    if (!providers || !showPackageHandOver.value) return []
    const kinds = sourceKinds(candidates)
    return accounts.value
      .filter(account => {
        if (!account.enabled) return false
        const slug = account.provider.toLowerCase()
        return (kinds.links && providers.has(slug)) || (kinds.torrentFiles && torrentProviders.value.has(slug))
      })
      .map(account => ({
        accountId: account.id,
        label: `${account.label} · ${providerName(account.provider)}`,
        provider: providerName(account.provider)
      }))
  }

  /** The provider a link that stayed after its hand-over went to, or `null`. */
  function handedOverTo(candidate: LinkCandidate): string | null {
    const mark = (candidate as LinkCandidate & { handed_over?: CandidateHandOverMark | null }).handed_over
    if (!mark) return null
    const account = accounts.value.find(entry => entry.id === mark.account_id)
    return account ? providerName(account.provider) : t('linkgrabber.package_hand_over.provider_fallback')
  }

  /** Hands every link of `item` to one account and reports what went and what stayed. */
  async function handOver(item: Pick<CollectorPackage, 'id' | 'name'>, accountId: string): Promise<void> {
    if (handingOverIds.value.has(item.id)) return
    const provider = accounts.value.find(account => account.id === accountId)?.provider
    const providerLabel = provider ? providerName(provider) : t('linkgrabber.package_hand_over.provider_fallback')
    handingOverIds.value = new Set([...handingOverIds.value, item.id])
    try {
      const answer = await handOverCollectorPackage(item.id, accountId)
      if (!answer.ok) {
        showError(t('linkgrabber.package_hand_over.failed', { name: item.name }), translateServerMessage(answer.message))
        return
      }
      const done = answer.data.handed_over.length
      const stayed = answer.data.stayed.length
      const summary = { name: item.name, provider: providerLabel, done, stayed, total: done + stayed }
      const first = answer.data.stayed[0]
      if (!first) {
        toast.add({ title: t('linkgrabber.package_hand_over.done', summary, done), color: 'success', icon: 'i-lucide-cloud-upload' })
      } else {
        // The first reason stands for the rest: a package to one account tends to stay for one cause.
        const reason = translateServerMessage({ code: first.code, message: first.message })
        const key = done ? 'linkgrabber.package_hand_over.partly' : 'linkgrabber.package_hand_over.nothing'
        toast.add({ title: t(key, summary), description: reason, color: 'warning', icon: 'i-lucide-circle-alert' })
      }
      // The `collector.changed` event refreshes the list too; this makes the change visible at once.
      void collector.refresh()
    } finally {
      const next = new Set(handingOverIds.value)
      next.delete(item.id)
      handingOverIds.value = next
    }
  }

  return { targets, handedOverTo, handOver, handingOverIds }
}
