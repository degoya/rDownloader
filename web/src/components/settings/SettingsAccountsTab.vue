<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import type {
  Account,
  AccountTest,
  AuthFlow,
  CreateAccount,
  CredentialMode,
  Provider,
  ProxyProfile,
  UpdateAccount
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import AccountBrowserSession from '@/components/settings/AccountBrowserSession.vue'
import { useBrowserSessions } from '@/composables/useBrowserSessions'
import { useConfirm } from '@/composables/useConfirm'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { providerText as pluginProviderText } from '@/i18n/plugins'
import { translateAccountLabel, translateServerMessage } from '@/i18n/server'
import { formatBytes } from '@/utils/format'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import { withPluginVersion } from '@/utils/pluginVersion'
import SectionHeader from '@/components/SectionHeader.vue'

/** The setup wizard embeds this tab under its own step heading. */
defineProps<{ hideHeader?: boolean }>()

const { t } = useI18n()
const accounts = ref<Account[]>([])
const proxies = ref<ProxyProfile[]>([])
const providers = ref<Provider[]>([])
const pending = ref(false)
/** The three parallel fetches below, as the list has to show them (RD-104-07). */
const { loading, loadError, load } = useFetchState()
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const editingAccountId = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const clearSecret = ref(false)
const clearCookies = ref(false)
const hostersByAccount = ref<Record<string, string[]>>({})
const hosterFilter = ref('')
const hostersLoadingId = ref<string | null>(null)
/** Last successful test per account id, so the outcome survives the transient alert. */
const testResults = ref<Record<string, AccountTest>>({})

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
const testingAccountId = ref<string | null>(null)
/**
 * Sessions asked of the browser extension (RD-120-45). One that arrives changes the account —
 * the cookie badge appears — and is checked at once, the way a saved account is.
 */
const browserSessions = useBrowserSessions((accountId) => {
  void refresh().then(() => {
    const account = accounts.value.find(candidate => candidate.id === accountId)
    if (account?.enabled) void testAccount(account, { quiet: true })
  })
})

/** The one site whose session the extension can hand over, when the provider declares one. */
function browserSessionHost(account: Account): string | null {
  return providers.value.find(provider => provider.slug === account.provider)?.cookie_scope_host ?? null
}
const togglingAccountId = ref<string | null>(null)
const deletingAccountId = ref<string | null>(null)
const confirm = useConfirm()

const accountForm = reactive<CreateAccount>({
  provider: 'ddownload',
  label: '',
  username: null,
  credential_mode: null,
  secret: null,
  cookies: null,
  proxy_profile_id: null,
  enabled: true
})

const proxyItems = computed(() => [
  { label: t('network.proxy.direct'), value: NO_SELECTION },
  ...proxies.value.map(proxy => ({ label: `${proxy.name} · ${proxy.kind}`, value: proxy.id }))
])
const accountProxySelection = computed({
  get: () => optionalSelection(accountForm.proxy_profile_id),
  set: (value: string) => { accountForm.proxy_profile_id = selectionValue(value) }
})
// A provider with `credentials: 'none'` resolves the free flow and stores nothing, so it has no
// place in a list whose only purpose is entering a credential (RD-098-01).
const accountProviders = computed(() => providers.value.filter(provider => provider.credentials !== 'none'))
const providerItems = computed(() =>
  accountProviders.value.map(provider => ({
    label: withPluginVersion(
      pluginProviderText(provider.slug, 'name') ?? provider.display_name,
      provider.plugin_version
    ),
    value: provider.slug
  }))
)
const selectedProvider = computed<Provider | undefined>(() => providers.value.find(provider => provider.slug === accountForm.provider))

/**
 * The credential modes the selected provider offers, empty when it offers no choice.
 *
 * Only such a provider shows the sign-in method picker; for everyone else there is exactly one
 * way to hold an account and asking would be noise.
 */
const credentialModes = computed<CredentialMode[]>(() => selectedProvider.value?.credential_modes ?? [])
const credentialModeItems = computed(() =>
  credentialModes.value.map(mode => ({ label: t(`network.account.credential_mode_${mode}`), value: mode }))
)

/**
 * Keeps the form's mode valid for whatever provider is selected.
 *
 * Switching to a provider that offers no choice clears it — the backend rejects a mode the
 * provider does not offer — and switching to one that does picks its first, which is the same
 * default the backend applies to an account that stores none.
 *
 * Watched on the offered modes rather than on `accountForm.provider`, and immediately (RD-109-35).
 * The provider never changes when the tab is first opened — it starts at its default — so a
 * watcher on it never ran at all, and the form opened with neither radio selected while the
 * secret field beneath was labelled for both modes at once. The catalogue arrives after mount,
 * so `{ immediate: true }` alone would only have run against an empty list; the modes themselves
 * change both when the fetch lands and when somebody picks another provider, which is exactly
 * the two moments the default has to be (re)established.
 */
watch(
  credentialModes,
  (modes) => {
    if (!modes.length) return void (accountForm.credential_mode = null)
    if (!accountForm.credential_mode || !modes.includes(accountForm.credential_mode)) {
      accountForm.credential_mode = modes[0] ?? null
    }
  },
  { immediate: true }
)

/**
 * Credential label or hint for a provider.
 *
 * Each provider's own wording ships inside its plugin package, so a third-party hoster gets
 * proper labels without a core release; anything it does not supply falls back to the
 * generic core text. A provider offering a choice of modes ships one label and hint per mode,
 * because the same field holds a password in one and an API key in the other.
 */
function credentialText(prefix: 'secret' | 'hint', slug: string): string {
  const key = prefix === 'secret' ? 'secret_label' : 'secret_hint'
  const mode = accountForm.credential_mode
  const shipped = (mode ? pluginProviderText(slug, `${key}_${mode}`) : null) ?? pluginProviderText(slug, key)
  if (shipped) return shipped
  if (prefix === 'secret') return genericCredentialNoun()
  if (selectedProvider.value?.credentials === 'oauth') return t('network.account.hint_oauth_client')
  return t('network.account.hint_generic')
}

/**
 * What to call the secret when the provider's plugin ships no wording of its own.
 *
 * The registry already records what a provider stores, so a provider that takes an account
 * password is not asked for an "API key". Anything unrecognised keeps the both-ways wording.
 */
function genericCredentialNoun(): string {
  const kind = selectedProvider.value?.credentials
  // With a choice of modes the mode decides, not the provider kind.
  if (accountForm.credential_mode === 'login') return t('network.account.secret_generic_password')
  if (accountForm.credential_mode === 'api_key') return t('network.account.secret_generic_api_key')
  if (kind === 'api_key') return t('network.account.secret_generic_api_key')
  if (kind === 'username_password') return t('network.account.secret_generic_password')
  // An OAuth account holds no password of ours: the field takes the secret of the person's
  // own registered client, and everything after that is fetched by the flow.
  if (kind === 'oauth') return t('network.account.secret_generic_oauth_client')
  return t('network.account.secret_generic')
}

/**
 * Sign-in flows a plugin is running, by account (RD-090-13).
 *
 * The service drives them; this only reads state and shows it. Closing the page in the middle
 * of a sign-in therefore loses nothing — reopening it picks the flow up where it stands.
 */
const authFlows = ref<Record<string, AuthFlow | null>>({})
const connectingAccountId = ref<string | null>(null)
let flowTimer: number | null = null

/** Whether an installed plugin can sign this account in instead of a key being typed. */
function canConnect(account: Account): boolean {
  const provider = providers.value.find(candidate => candidate.slug === account.provider)
  if (!provider?.device_flow) return false
  // Premiumize's device flow produces the same API key that can be entered by hand. Once that
  // slot is filled, offering "Connect" again is duplicate UI. OAuth providers are different:
  // their account secret may be the client secret needed to start the flow, so `has_secret`
  // must not hide their sign-in action.
  return provider.credentials !== 'api_key' || !account.has_secret
}

function flowOf(account: Account): AuthFlow | null {
  return authFlows.value[account.id] ?? null
}

/** What to call a provider: its plugin's own name, the registry's, or the bare slug. */
function providerName(slug: string): string {
  return pluginProviderText(slug, 'name')
    ?? providers.value.find(provider => provider.slug === slug)?.display_name
    ?? slug
}

/**
 * Why a flow ended, in the reader's language (RD-106-02).
 *
 * Two kinds of text arrive in `message`. What the service decided by itself — no installed
 * plugin claims this provider, the provider stayed unreachable — is a stable code, because
 * there is no foreign answer to quote and a code can be translated. What a plugin reported
 * is the provider's own English wording, which nothing here can translate.
 * `translateServerMessage` takes the first and falls through to the second.
 */
function flowMessage(account: Account): string {
  const message = flowOf(account)?.message
  if (!message) return t('network.account.connect_failed')
  return translateServerMessage({
    code: message,
    message,
    params: { provider: providerName(account.provider) }
  })
}

async function loadFlow(accountId: string): Promise<void> {
  const response = await api.GET('/api/v1/accounts/{id}/auth', { params: { path: { id: accountId } } })
  authFlows.value = { ...authFlows.value, [accountId]: response.data ?? null }
}

/** Starts a flow and begins watching it; the address is shown, never opened for somebody. */
async function connectAccount(account: Account): Promise<void> {
  connectingAccountId.value = account.id
  error.value = null
  message.value = null
  const response = await api.POST('/api/v1/accounts/{id}/auth/begin', {
    params: { path: { id: account.id } }
  })
  connectingAccountId.value = null
  if (!response.data) return void (error.value = responseError(response))
  authFlows.value = { ...authFlows.value, [account.id]: response.data }
  watchFlows()
}

async function cancelConnect(account: Account): Promise<void> {
  await api.DELETE('/api/v1/accounts/{id}/auth', { params: { path: { id: account.id } } })
  authFlows.value = { ...authFlows.value, [account.id]: null }
}

/** Polls the flows that are still open, and stops as soon as none are. */
function watchFlows(): void {
  if (flowTimer !== null) return
  flowTimer = window.setInterval(() => {
    const open = Object.entries(authFlows.value).filter(
      ([, flow]) => flow && (flow.state === 'waiting_for_user' || flow.state === 'polling')
    )
    if (!open.length) {
      window.clearInterval(flowTimer ?? 0)
      flowTimer = null
      return
    }
    void Promise.all(open.map(([id]) => loadFlow(id))).then(() => {
      // A finished sign-in changes the account: the key badge appears once it is stored.
      if (Object.values(authFlows.value).some(flow => flow?.state === 'authorized')) void refresh()
    })
  }, 3000)
}

onUnmounted(() => {
  if (flowTimer !== null) window.clearInterval(flowTimer)
  releaseEvents?.()
  releaseEvents = null
  if (providerTimer !== null) {
    window.clearTimeout(providerTimer)
    providerTimer = null
  }
})

const showSecretInput = computed(() => selectedProvider.value?.credentials !== 'cookies')
/** Signing in makes the account's own credentials the session, so a username is required. */
const usernameRequired = computed(
  () => selectedProvider.value?.username_required === true || accountForm.credential_mode === 'login'
)
/**
 * Whether to ask for a pasted cookie session at all.
 *
 * In `login` mode there is nothing to paste — that is the entire point of the mode — so the
 * field would only invite the very copy-and-paste it removes.
 */
const showCookiesInput = computed(() => accountForm.credential_mode !== 'login')
/** The provider's own word for its secret, used as the field label and in the edit hint. */
const credentialNoun = computed(() => credentialText('secret', accountForm.provider))

const secretPlaceholder = computed(() => {
  if (editingAccountId.value) {
    return t('network.account.secret_keep', { credential: credentialNoun.value })
  }
  return credentialNoun.value
})
const cookiesPlaceholder = computed(() => editingAccountId.value ? t('network.account.cookies_keep') : t('network.account.cookies_placeholder'))
const credentialHint = computed(() => credentialText('hint', accountForm.provider))
/** The live subscription and the timer that coalesces a burst of plugin events into one read. */
let releaseEvents: (() => void) | null = null
let providerTimer: number | null = null

onMounted(() => {
  void load(refresh).then(() => {
    // A flow may have been running when the page was last closed; picking it up is what
    // makes closing the browser mid-sign-in cost nothing.
    void Promise.all(accounts.value.map(account => loadFlow(account.id))).then(watchFlows)
  })
  releaseEvents = subscribeEvents({ 'plugin_catalog.changed': scheduleProviderReload })
})

/**
 * What this tab does when the bus says the installed plugins changed.
 *
 * The provider catalogue is filled solely from installed plugin manifests, and this tab reads
 * it once on mount. A resolver installed, removed, enabled or disabled elsewhere therefore left
 * the provider picker offering a provider that no longer exists — and an account created
 * against it fails at the first resolution — or hiding one that had just become available. The
 * `device_flow` flag has the same problem in the other direction: it says whether an
 * authentication plugin can sign this provider in, so removing that plugin turns a "sign in"
 * button into one that cannot work.
 *
 * The channel is `plugin_catalog.changed`, not `plugin.changed`: the catalogue is read from
 * `/api/v1/providers`, which costs `Config`, and an event reaches a subscriber only when it
 * holds that event's exact scope. `plugin.changed` carries the same payload for the `Admin`
 * inventory, and a `Config` token never sees it.
 *
 * Only the catalogue is re-read, not the whole tab. Accounts and proxy profiles are database
 * rows that a plugin event says nothing about, and each has its own event; re-reading them here
 * would put this tab's list at the mercy of an unrelated failure. Refetched rather than patched
 * because the event carries no catalogue, and this list is one the service computes.
 *
 * `load()` is deliberately bypassed: it sets `loading` on every call, so an arriving event
 * would trade the account list for its skeleton and back (RD-106-19). A failed read leaves the
 * previous catalogue standing and is recorded the way this tab's other failures are. No notice
 * is raised — `design.md` has no pattern for announcing that data caught up.
 */
function scheduleProviderReload(): void {
  if (providerTimer !== null) return
  providerTimer = window.setTimeout(() => {
    providerTimer = null
    void refreshProviders()
  }, 300)
}

async function refreshProviders(): Promise<void> {
  const response = await api.GET('/api/v1/providers')
  if (response.data) providers.value = response.data
  else error.value = responseError(response)
}

/**
 * Fetches the tab's three lists, returning the failure rather than swallowing it.
 *
 * Opening the tab used to read "no accounts" for the whole duration of these three requests,
 * and again for good if any of them failed. The returned message is what tells the list which
 * of the two it is looking at.
 */
async function refresh(): Promise<string | null> {
  const [accountResponse, proxyResponse, providerResponse] = await Promise.all([
    api.GET('/api/v1/accounts'),
    api.GET('/api/v1/proxy-profiles'),
    api.GET('/api/v1/providers')
  ])
  if (proxyResponse.data) proxies.value = proxyResponse.data
  if (providerResponse.data) providers.value = providerResponse.data
  if (!accountResponse.data) return responseError(accountResponse)
  accounts.value = accountResponse.data
  return null
}

async function createAccount(): Promise<void> {
  pending.value = true
  error.value = null
  message.value = null
  const body: CreateAccount = {
    ...accountForm,
    username: accountForm.username || null,
    secret: accountForm.secret || null,
    cookies: accountForm.cookies || null,
    proxy_profile_id: accountForm.proxy_profile_id || null
  }
  if (editingAccountId.value) {
    const updateBody: UpdateAccount = {
      ...body,
      clear_secret: clearSecret.value,
      clear_cookies: clearCookies.value
    }
    const response = await api.PUT('/api/v1/accounts/{id}', {
      params: { path: { id: editingAccountId.value } },
      body: updateBody
    })
    pending.value = false
    if (!response.data) return void (error.value = responseError(response))
    accounts.value = accounts.value.map(account => account.id === response.data!.id ? response.data! : account)
    message.value = t('network.messages.updated')
    resetAccountForm()
    return
  }
  const response = await api.POST('/api/v1/accounts', { body })
  pending.value = false
  if (!response.data) return void (error.value = responseError(response))
  accounts.value.push(response.data)
  message.value = t('network.messages.created')
  checkAfterSaving(response.data)
  resetAccountForm()
}

function editAccount(account: Account): void {
  error.value = null
  message.value = null
  editingAccountId.value = account.id
  clearTestResult(account.id)
  accountForm.provider = account.provider
  accountForm.label = account.label
  accountForm.username = account.username ?? null
  // An account written before its provider offered a choice stores none; the first offered
  // mode is what the backend falls back to for it, so the form shows the same.
  accountForm.credential_mode = account.credential_mode ?? credentialModes.value[0] ?? null
  accountForm.secret = null
  accountForm.cookies = null
  accountForm.proxy_profile_id = account.proxy_profile_id ?? null
  accountForm.enabled = account.enabled
  clearSecret.value = false
  clearCookies.value = false
  void focusForm()
}

function resetAccountForm(): void {
  editingAccountId.value = null
  accountForm.label = ''
  accountForm.username = null
  accountForm.credential_mode = credentialModes.value[0] ?? null
  accountForm.secret = null
  accountForm.cookies = null
  accountForm.proxy_profile_id = null
  accountForm.enabled = true
  clearSecret.value = false
  clearCookies.value = false
}

/// Switches one account on or off from the list.
///
/// Uses the ordinary update endpoint rather than a pair of routes of its own: `update_account`
/// already leaves the vault references untouched when no secret is sent, and a second write path
/// would be one more place that could get that wrong.
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

function accountTestMessage(account: Account, result: AccountTest): string {
  const label = translateAccountLabel(result.label)
  const parts = [
    ...(label ? [label] : []),
    result.premium ? t('network.messages.premium_active') : t('network.messages.premium_inactive')
  ]
  if (result.traffic_left) parts.push(t('network.messages.traffic_left', { amount: formatBytes(result.traffic_left) }))
  return `${account.label}: ${parts.join(' · ')}`
}

function proxyName(id: string | null | undefined): string {
  return proxies.value.find(proxy => proxy.id === id)?.name ?? t('network.proxy.direct_short')
}
</script>

<template>
  <div class="w-full space-y-6">
    <header v-if="!hideHeader">
      <SectionHeader
        :eyebrow="t('settings.headers.accounts.eyebrow')"
        :title="t('settings.headers.accounts.title')"
        :description="t('settings.headers.accounts.description')"
        level="page"
      />
    </header>
    <UAlert v-if="error" color="error" variant="subtle" :description="error" />
    <UAlert v-if="browserSessions.error.value" color="error" variant="subtle" :description="browserSessions.error.value" />
    <UAlert v-if="message" color="success" variant="subtle" :description="message" />

    <section class="border border-muted bg-default p-5">
      <FormListLayout :list-title="t('network.account.title')" :count="accounts.length">
        <template #form>
          <SectionHeader
            :eyebrow="t('network.account.eyebrow')"
            :title="editingAccountId ? t('network.account.edit_title') : t('network.account.new_title')"
          />
          <form ref="formElement" class="mt-4 grid gap-3" @submit.prevent="createAccount">
            <UFormField :label="t('network.account.provider_label')">
              <USelect v-model="accountForm.provider" :items="providerItems" class="w-full" />
            </UFormField>
            <!--
              The sign-in method stands directly under the provider, because it decides what the
              rest of this form asks for: the username becomes required, the cookie session
              disappears, and the secret changes from a password to an API key. Filling the form
              top to bottom must not mean answering three questions before the one that says
              whether they exist (RD-109-35).
            -->
            <div v-if="credentialModeItems.length">
              <p class="mb-1 text-xs text-muted">{{ t('network.account.credential_mode') }}</p>
              <URadioGroup v-model="accountForm.credential_mode" orientation="horizontal" :items="credentialModeItems" />
            </div>
            <UFormField :label="t('network.account.label_label')" required>
              <UInput v-model="accountForm.label" required maxlength="100" class="w-full" :placeholder="t('network.account.label_placeholder')" />
            </UFormField>
            <UFormField :label="t('network.account.username_label')" :required="usernameRequired">
              <UInput v-model="accountForm.username" :required="usernameRequired" class="w-full" :placeholder="t('network.account.username_placeholder')" />
            </UFormField>
            <UFormField :label="t('network.account.proxy_label')">
              <USelect v-model="accountProxySelection" :items="proxyItems" class="w-full" :placeholder="t('network.account.proxy_placeholder')" />
            </UFormField>
            <UFormField v-if="showSecretInput" :label="credentialNoun">
              <UInput v-model="accountForm.secret" type="password" class="w-full" :aria-label="credentialNoun" :placeholder="secretPlaceholder" />
            </UFormField>
            <UFormField v-if="showCookiesInput" :label="t('network.account.cookies_label')">
              <UTextarea v-model="accountForm.cookies" :rows="3" autoresize class="w-full font-mono text-xs" :placeholder="cookiesPlaceholder" />
            </UFormField>
            <label v-if="editingAccountId && showSecretInput" class="flex items-center gap-3 text-xs text-muted"><USwitch v-model="clearSecret" /> {{ t('network.account.clear_secret') }}</label>
            <label v-if="editingAccountId" class="flex items-center gap-3 text-xs text-muted"><USwitch v-model="clearCookies" /> {{ t('network.account.clear_cookies') }}</label>
            <label class="flex items-center gap-3 text-sm text-muted"><USwitch v-model="accountForm.enabled" /> {{ t('network.account.enabled') }}</label>
            <div class="flex gap-2">
              <UButton type="submit" :icon="editingAccountId ? 'i-lucide-save' : 'i-lucide-user-plus'" :label="editingAccountId ? t('network.account.save_changes') : t('network.account.create')" :loading="pending" />
              <UButton v-if="editingAccountId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('network.account.cancel_edit')" @click="resetAccountForm" />
            </div>
          </form>
          <UAlert
            class="mt-3"
            color="info"
            variant="subtle"
            :title="t('network.account.hint_title')"
            :description="credentialHint"
          />
          <p v-if="showCookiesInput" class="mt-2 text-xs leading-5 text-warning">{{ t('network.account.cookies_warning') }}</p>
        </template>
        <template #list>
          <div class="grid gap-2">
            <div v-for="account in accounts" :key="account.id" class="border p-3" :class="editingAccountId === account.id ? 'border-primary' : 'border-muted'">
              <div class="flex items-center gap-3">
                <USwitch
                  :model-value="account.enabled"
                  :disabled="togglingAccountId === account.id"
                  :aria-label="account.enabled ? t('network.account.disable') : t('network.account.enable')"
                  :title="account.enabled ? t('network.account.disable') : t('network.account.enable')"
                  @update:model-value="(value: boolean) => setAccountEnabled(account, value)"
                />
                <div class="min-w-0 flex-1"><p class="text-sm font-medium text-highlighted">{{ account.label }}</p><p class="font-mono text-[11px] text-muted">{{ account.provider }} · {{ proxyName(account.proxy_profile_id) }}</p></div>
                <UBadge v-if="editingAccountId === account.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                <UIcon v-if="account.has_secret" name="i-lucide-key-round" class="text-primary" />
                <UIcon v-if="account.has_cookies" name="i-lucide-cookie" class="text-warning" />
                <UBadge
                  v-if="testResults[account.id]"
                  :color="testResults[account.id]!.premium ? 'success' : 'neutral'"
                  variant="subtle"
                  size="sm"
                  :title="t('network.account.last_test')"
                >
                  {{ testBadge(account.id) }}
                </UBadge>
                <UButton
                  v-if="canConnect(account)"
                  size="xs"
                  color="primary"
                  variant="ghost"
                  icon="i-lucide-link"
                  :label="t('network.account.connect')"
                  :loading="connectingAccountId === account.id"
                  @click="connectAccount(account)"
                />
                <UButton
                  v-if="browserSessionHost(account)"
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-cookie"
                  :label="t('network.account.browser_session.take_over')"
                  :title="t('network.account.browser_session.take_over_hint', { host: browserSessionHost(account) })"
                  :loading="browserSessions.startingId.value === account.id"
                  @click="browserSessions.begin(account.id)"
                />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('network.account.test')" :loading="testingAccountId === account.id" @click="testAccount(account)" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('network.account.edit')" @click="editAccount(account)" />
                <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('network.account.delete')" :loading="deletingAccountId === account.id" @click="deleteAccount(account)" />
              </div>
              <div v-if="flowOf(account)" class="mt-2 border border-muted bg-elevated p-3">
                <template v-if="flowOf(account)!.state === 'waiting_for_user' || flowOf(account)!.state === 'polling'">
                  <p class="text-xs leading-5 text-muted">{{ t('network.account.connect_instructions') }}</p>
                  <p class="mt-2 break-all font-mono text-sm text-highlighted">{{ flowOf(account)!.verification_url }}</p>
                  <p v-if="flowOf(account)!.user_code" class="mt-1 font-mono text-lg font-semibold tracking-widest text-primary">
                    {{ flowOf(account)!.user_code }}
                  </p>
                  <p class="mt-2 text-xs leading-5 text-muted">{{ t('network.account.connect_waiting') }}</p>
                  <UButton class="mt-2" size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('network.account.connect_cancel')" @click="cancelConnect(account)" />
                </template>
                <p v-else-if="flowOf(account)!.state === 'authorized'" class="text-xs leading-5 text-success">
                  {{ t('network.account.connect_done') }}
                </p>
                <p v-else class="text-xs leading-5 text-error">
                  {{ flowMessage(account) }}
                </p>
              </div>
              <AccountBrowserSession
                v-if="browserSessions.sessions.value[account.id]"
                :session="browserSessions.sessions.value[account.id]!"
                @cancel="browserSessions.cancel(account.id)"
                @dismiss="browserSessions.dismiss(account.id)"
              />
              <UCollapsible class="mt-2" @update:open="(open: boolean) => open && loadHosters(account.id)">
                <UButton size="xs" color="neutral" variant="ghost" class="w-full justify-between" icon="i-lucide-list-checks" :label="t('network.hosters.toggle')" trailing-icon="i-lucide-chevron-down" />
                <template #content>
                  <div class="border-t border-muted pt-2">
                    <p class="mb-2 text-xs leading-5 text-muted">{{ t('network.hosters.description') }}</p>
                    <UInput v-model="hosterFilter" icon="i-lucide-search" size="sm" :placeholder="t('network.hosters.filter_placeholder')" class="mb-2 w-full" />
                    <p v-if="hostersLoadingId === account.id" class="text-xs text-muted">{{ t('network.hosters.loading') }}</p>
                    <p v-else-if="!(hostersByAccount[account.id]?.length)" class="text-xs text-muted">{{ t('network.hosters.empty') }}</p>
                    <div v-else class="flex max-h-48 flex-wrap gap-1 overflow-y-auto">
                      <UBadge v-for="host in visibleHosters(account.id)" :key="host" color="neutral" variant="subtle" size="sm" class="font-mono">{{ host }}</UBadge>
                    </div>
                  </div>
                </template>
              </UCollapsible>
            </div>
            <DataState :loading="loading" :error="loadError" :empty="!accounts.length">
              <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('network.account.empty') }}</p>
            </DataState>
          </div>
        </template>
      </FormListLayout>
    </section>
  </div>
</template>
