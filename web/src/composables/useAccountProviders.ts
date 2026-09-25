import { ref } from 'vue'

import { api } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'

const providersWithAccount = ref(new Set<string>())
const registryProviders = ref(new Set<string>())
const hasMultihosterAccount = ref(false)
/** Every provider's display name by slug, from the same `/api/v1/providers` read (RD-130-11). */
const displayNames = ref(new Map<string, string>())
let loaded = false
/** Coalesces a burst of plugin events into one pair of reads. */
let refreshTimer: number | null = null

async function refresh(): Promise<void> {
  const [accountsResponse, providersResponse] = await Promise.all([
    api.GET('/api/v1/accounts'),
    api.GET('/api/v1/providers')
  ])
  const enabled = (accountsResponse.data ?? []).filter(account => account.enabled)
  providersWithAccount.value = new Set(enabled.map(account => account.provider.toLowerCase()))
  const providers = providersResponse.data ?? []
  // Providers that take no account are left out: `lacksAccount` would otherwise flag every
  // one of their links as needing an account nobody can create (RD-098-01).
  registryProviders.value = new Set(
    providers.filter(provider => provider.credentials !== 'none').map(provider => provider.slug)
  )
  displayNames.value = new Map(providers.map(provider => [provider.slug.toLowerCase(), provider.display_name]))
  const multihosters = new Set(providers.filter(provider => provider.kind === 'multihoster').map(provider => provider.slug))
  hasMultihosterAccount.value = enabled.some(account => multihosters.has(account.provider.toLowerCase()))
}

/**
 * What this composable does when the bus says the installed plugins changed.
 *
 * The provider registry is filled solely from installed plugin manifests, so installing,
 * removing, enabling or disabling a resolver changes `registryProviders` — and with it whether
 * a candidate row is flagged as lacking an account. Read once at first use and never again,
 * this set kept flagging links for a hoster whose plugin had just been removed, and kept
 * missing the ones a freshly installed plugin had just made known, until the page was reloaded.
 *
 * The channel is `plugin_catalog.changed`, not `plugin.changed`: this composable reads
 * `/api/v1/providers`, which costs `Config`, and a subscriber is handed an event only when it
 * holds that event's exact scope. `plugin.changed` carries the same payload at `Admin`, so a
 * `Config` token would have been subscribed to a channel it can never be delivered.
 *
 * Refetched rather than patched: the event says only that something about the plugins changed,
 * and the catalogue is not derivable here — the service answers `credentials`, `kind` and the
 * `device_flow` flag from what is installed right now, and the account half has to be re-read
 * with it because whether a multihoster account covers a link depends on both lists agreeing.
 * `refresh()` sets no loading flag of its own, so an arriving event cannot blank a row's badge
 * and bring it back; the flags simply become correct. Debounced, because installing a package
 * emits more than one event. No notice is raised — `design.md` has no pattern for announcing
 * that data caught up.
 */
function scheduleRefresh(): void {
  if (refreshTimer !== null) return
  refreshTimer = window.setTimeout(() => {
    refreshTimer = null
    void refresh()
  }, 300)
}

/**
 * Which providers have an enabled account — used to flag hoster links that will be fetched
 * as a free/direct download. Loaded once and shared; call `refresh()` after account changes.
 */
export function useAccountProviders() {
  if (!loaded) {
    loaded = true
    void refresh()
    // Never released, deliberately. This state is module-level and shared by every candidate
    // row, so it outlives all of them and there is no unmount at which dropping it would be
    // correct; the one subscription costs nothing beyond the stream the app already holds.
    // `account.changed` for the same reason: half of what this answers is which providers have
    // an enabled account, and an account added, removed or switched off anywhere else left that
    // half as stale as an uninstalled plugin left the other.
    subscribeEvents({ 'plugin_catalog.changed': scheduleRefresh, 'account.changed': scheduleRefresh })
  }

  /**
   * `true` when the provider is a known hoster with neither its own account nor a multihoster
   * account that might cover it. Non-registry providers (direct_http, media, …) never match.
   */
  function lacksAccount(provider: string | null | undefined): boolean {
    if (!provider) return false
    const slug = provider.toLowerCase()
    if (!registryProviders.value.has(slug)) return false
    if (providersWithAccount.value.has(slug)) return false
    return !hasMultihosterAccount.value
  }

  /**
   * The name a provider is shown under, for a slug the server stored — the one that answered a
   * cache check (RD-130-11). Falls back to the slug itself while the catalogue is loading, or
   * for a provider whose plugin has since been removed.
   */
  function providerName(slug: string): string {
    return displayNames.value.get(slug.toLowerCase()) ?? slug
  }

  return { lacksAccount, providerName, refresh }
}
