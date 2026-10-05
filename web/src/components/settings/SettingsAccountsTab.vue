<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import type { Account, CreateAccount, Provider, ProxyProfile, UpdateAccount } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import AccountBrowserSession from '@/components/settings/AccountBrowserSession.vue'
import AccountSignInFlow from '@/components/settings/AccountSignInFlow.vue'
import { signsInWithCode, useAccountForm } from '@/composables/useAccountForm'
import { useAccountHosters } from '@/composables/useAccountHosters'
import { useAccountTests } from '@/composables/useAccountTests'
import { isOpenFlow, useAuthFlows } from '@/composables/useAuthFlows'
import { useBrowserSessions } from '@/composables/useBrowserSessions'
import { useConfirm } from '@/composables/useConfirm'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { providerText as pluginProviderText } from '@/i18n/plugins'
import SectionHeader from '@/components/SectionHeader.vue'

/** The setup wizard embeds this tab under its own step heading. */
defineProps<{ hideHeader?: boolean }>()

const { t } = useI18n()
const accounts = ref<Account[]>([])
const proxies = ref<ProxyProfile[]>([])
const providers = ref<Provider[]>([])
/**
 * The provider catalogue's first read, apart from the account list's (RD-130-06). It is
 * answered from the installed plugins, and while the service is still loading those the answer
 * can take far longer than the accounts — which it used to hold back with it.
 */
const providersLoading = ref(true)
const pending = ref(false)
/** The three parallel fetches below, as the list has to show them (RD-104-07). */
const { loading, loadError, load } = useFetchState()
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const { hostersByAccount, hosterFilter, hostersLoadingId, visibleHosters, loadHosters } = useAccountHosters()
const { testResults, testingAccountId, clearTestResult, testBadge, checkAfterSaving, testAccount } =
  useAccountTests(error, message)
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

const {
  accountForm,
  editingAccountId,
  clearSecret,
  clearCookies,
  proxyItems,
  accountProxySelection,
  providerItems,
  credentialModeItems,
  showSecretInput,
  usernameRequired,
  showCookiesInput,
  credentialNoun,
  secretPlaceholder,
  cookiesPlaceholder,
  credentialHint,
  fillFrom,
  resetAccountForm
} = useAccountForm(providers, proxies)

/** The mode an account is held in, filling in the provider's default like the backend does. */
function accountMode(account: Account): string | null {
  const provider = providers.value.find(candidate => candidate.slug === account.provider)
  return account.credential_mode ?? provider?.credential_modes?.[0] ?? null
}

/** Sign-in flows by account; one that finishes changes the account, so the list is re-read. */
const authFlows = useAuthFlows(() => { void refresh() })
const { flowOf } = authFlows

/** Whether an installed plugin can sign this account in instead of a key being typed. */
function canConnect(account: Account): boolean {
  const provider = providers.value.find(candidate => candidate.slug === account.provider)
  if (!provider?.device_flow) return false
  // A provider offering a sign-in with a code beside a typed key signs in only accounts held in
  // the first way; one holding the typed key has nothing a sign-in would add (RD-150-09).
  if (provider.credential_modes?.length) return signsInWithCode(accountMode(account))
  // Premiumize's device flow produces the same API key that can be entered by hand. Once that
  // slot is filled, offering "Connect" again is duplicate UI. OAuth providers are different:
  // their account secret may be the client secret needed to start the flow, so `has_secret`
  // must not hide their sign-in action.
  return provider.credentials !== 'api_key' || !account.has_secret
}

/** What to call a provider: its plugin's own name, the registry's, or the bare slug. */
function providerName(slug: string): string {
  return pluginProviderText(slug, 'name')
    ?? providers.value.find(provider => provider.slug === slug)?.display_name
    ?? slug
}

/** Shows the running sign-in or starts one (RD-150-09); only a failed start is reported. */
async function connectAccount(account: Account): Promise<void> {
  error.value = null
  message.value = null
  const failure = await authFlows.connect(account.id)
  if (failure) error.value = failure
}

onMounted(() => {
  void refreshProviders().finally(() => { providersLoading.value = false })
  void load(refresh).then(() => {
    // A flow may have been running when the page was last closed; picking it up is what
    // makes closing the browser mid-sign-in cost nothing.
    void Promise.all(accounts.value.map(account => authFlows.load(account.id)))
  })
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
useDebouncedEventRefresh(['plugin_catalog.changed'], refreshProviders, {
  // Every step a sign-in takes is recorded as an account change (RD-150-09).
  handlers: { 'account.changed': authFlows.onAccountEvent }
})

async function refreshProviders(): Promise<void> {
  const response = await api.GET('/api/v1/providers')
  if (response.data) providers.value = response.data
  else error.value = responseError(response)
}

/**
 * Fetches the accounts and proxy profiles, returning the failure rather than swallowing it.
 *
 * Opening the tab used to read "no accounts" for the whole duration of these requests, and
 * again for good if any of them failed. The returned message is what tells the list which of
 * the two it is looking at. The provider catalogue is read on its own (`providersLoading`).
 */
async function refresh(): Promise<string | null> {
  const [accountResponse, proxyResponse] = await Promise.all([
    api.GET('/api/v1/accounts'),
    api.GET('/api/v1/proxy-profiles')
  ])
  if (proxyResponse.data) proxies.value = proxyResponse.data
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
  // An account that signs in with a code has nothing to check yet: the code is what it needs,
  // so the sign-in starts at once and the address and code appear on its row (RD-150-09).
  // Started before the notice is set, because starting one clears the notices it replaces.
  if (signsInWithCode(response.data.credential_mode) && canConnect(response.data)) void connectAccount(response.data)
  else checkAfterSaving(response.data)
  message.value = t('network.messages.created')
  resetAccountForm()
}

function editAccount(account: Account): void {
  error.value = null
  message.value = null
  clearTestResult(account.id)
  fillFrom(account)
  void focusForm()
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
    <UCard as="section" data-settings-anchor="accounts.list">
      <FormListLayout :list-title="t('network.account.title')" :count="accounts.length">
        <template #form>
          <SectionHeader
            :eyebrow="t('network.account.eyebrow')"
            :title="editingAccountId ? t('network.account.edit_title') : t('network.account.new_title')"
          />
          <UAlert v-if="error" class="mt-4" color="error" variant="subtle" :description="error" />
          <UAlert v-if="browserSessions.error.value" class="mt-4" color="error" variant="subtle" :description="browserSessions.error.value" />
          <UAlert v-if="message" class="mt-4" color="success" variant="subtle" :description="message" />
          <form ref="formElement" class="mt-4 grid gap-3" @submit.prevent="createAccount">
            <!-- A picker whose options are on their way is not drawn (design.md). -->
            <UFormField :label="t('network.account.provider_label')">
              <DataState
                v-if="providersLoading"
                loading
                variant="inline"
                :rows="1"
                :label="t('network.account.providers_loading')"
                data-testid="account-providers-loading"
              />
              <UAlert
                v-else-if="!providerItems.length"
                color="warning"
                variant="subtle"
                icon="i-lucide-puzzle"
                :description="t('network.account.no_providers')"
                data-testid="account-providers-empty"
              />
              <USelect v-else v-model="accountForm.provider" :items="providerItems" class="w-full" />
            </UFormField>
            <!--
              The sign-in method stands directly under the provider, because it decides what the
              rest of this form asks for: the username becomes required, the cookie session
              disappears, and the secret changes from a password to an API key. Filling the form
              top to bottom must not mean answering three questions before the one that says
              whether they exist (RD-109-35).
            -->
            <URadioGroup
              v-if="credentialModeItems.length"
              v-model="accountForm.credential_mode"
              :legend="t('network.account.credential_mode')"
              orientation="horizontal"
              :items="credentialModeItems"
            />
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
            <USwitch v-if="editingAccountId && showSecretInput" v-model="clearSecret" size="sm" :label="t('network.account.clear_secret')" />
            <USwitch v-if="editingAccountId" v-model="clearCookies" size="sm" :label="t('network.account.clear_cookies')" />
            <USwitch v-model="accountForm.enabled" :label="t('network.account.enabled')" />
            <FormActions
              :editing="editingAccountId !== null"
              :create-label="t('network.account.create')"
              create-icon="i-lucide-user-plus"
              :save-label="t('network.account.save_changes')"
              :loading="pending"
              @cancel="resetAccountForm"
            />
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
            <div v-for="account in accounts" :key="account.id" class="min-w-0 border p-3" :class="editingAccountId === account.id ? 'border-primary' : 'border-muted'">
              <!-- Wraps: in a narrow column (the setup wizard) the buttons would otherwise push the
                   row, and with it the whole list, past the column's edge. -->
              <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
                <USwitch
                  :model-value="account.enabled"
                  :disabled="togglingAccountId === account.id"
                  :aria-label="account.enabled ? t('network.account.disable') : t('network.account.enable')"
                  :title="account.enabled ? t('network.account.disable') : t('network.account.enable')"
                  @update:model-value="(value: boolean) => setAccountEnabled(account, value)"
                />
                <div class="min-w-0 flex-1 basis-40"><p class="truncate text-sm font-medium text-highlighted">{{ account.label }}</p><p class="truncate font-mono text-[11px] text-muted">{{ account.provider }} · {{ proxyName(account.proxy_profile_id) }}</p></div>
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
                <!-- Not offered while a sign-in runs: a second one would replace its code. -->
                <UButton
                  v-if="canConnect(account) && !isOpenFlow(flowOf(account.id))"
                  size="xs"
                  color="primary"
                  variant="ghost"
                  icon="i-lucide-link"
                  :label="t('network.account.connect')"
                  :loading="authFlows.connectingId.value === account.id"
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
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('network.account.edit')" :title="t('network.account.edit')" @click="editAccount(account)" />
                <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('network.account.delete')" :title="t('network.account.delete')" :loading="deletingAccountId === account.id" @click="deleteAccount(account)" />
              </div>
              <AccountSignInFlow
                v-if="flowOf(account.id)"
                :flow="flowOf(account.id)!"
                :provider="providerName(account.provider)"
                @cancel="authFlows.cancel(account.id)"
              />
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
    </UCard>
  </div>
</template>
