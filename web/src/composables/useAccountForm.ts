import { computed, reactive, ref, watch, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account, CreateAccount, CredentialMode, Provider, ProxyProfile } from '@/api/types'
import { providerText as pluginProviderText } from '@/i18n/plugins'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import { withPluginVersion } from '@/utils/pluginVersion'

/**
 * Whether a credential mode signs the account in with a code rather than holding something
 * typed (RD-150-09, Real-Debrid's "Connect with a code").
 *
 * Takes a plain string on purpose: the mode arrives from the generated API types, and a literal
 * compared against a union that predates it would read as a mistake to the type checker.
 */
export function signsInWithCode(mode: string | null | undefined): boolean {
  return mode === 'oauth'
}

/**
 * The account form of the accounts tab: its values, and every field that depends on the
 * selected provider and sign-in method.
 */
export function useAccountForm(providers: Ref<Provider[]>, proxies: Ref<ProxyProfile[]>) {
  const { t } = useI18n()
  const editingAccountId = ref<string | null>(null)
  const clearSecret = ref(false)
  const clearCookies = ref(false)

  const accountForm = reactive<CreateAccount>({
    // Filled with the first installed provider once the list is loaded; there is no default
    // provider of its own, since which ones exist depends on the installed plugins.
    provider: '',
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
  // A new account starts at the first provider the installed plugins offer, and never keeps one
  // that is not (or no longer) in the list.
  watch(providerItems, (items) => {
    if (editingAccountId.value) return
    if (!items.some(item => item.value === accountForm.provider)) accountForm.provider = items[0]?.value ?? ''
  }, { immediate: true })
  const selectedProvider = computed<Provider | undefined>(() => providers.value.find(provider => provider.slug === accountForm.provider))

  /**
   * The credential modes the selected provider offers, empty when it offers no choice.
   *
   * Only such a provider shows the sign-in method picker; for everyone else there is exactly one
   * way to hold an account and asking would be noise.
   */
  const credentialModes = computed<CredentialMode[]>(() => selectedProvider.value?.credential_modes ?? [])
  /**
   * Each mode named the provider's way when its plugin ships a name, e.g. Real-Debrid's "API token"
   * where the core only knows the generic "API key".
   */
  const credentialModeItems = computed(() =>
    credentialModes.value.map(mode => ({
      label:
        pluginProviderText(accountForm.provider, `mode_label_${mode}`)
        ?? t(`network.account.credential_mode_${mode}`),
      value: mode
    }))
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

  /** Nothing is typed for a cookie-only provider, nor for an account that signs in with a code. */
  const showSecretInput = computed(
    () => selectedProvider.value?.credentials !== 'cookies' && !signsInWithCode(accountForm.credential_mode)
  )
  /** Signing in makes the account's own credentials the session, so a username is required. */
  const usernameRequired = computed(
    () => selectedProvider.value?.username_required === true || accountForm.credential_mode === 'login'
  )
  /**
   * Whether to ask for a pasted cookie session at all.
   *
   * In `login` mode there is nothing to paste — that is the entire point of the mode — so the
   * field would only invite the very copy-and-paste it removes. A sign-in with a code is the same
   * promise, made by the provider rather than by rDownloader.
   */
  const showCookiesInput = computed(
    () => accountForm.credential_mode !== 'login' && !signsInWithCode(accountForm.credential_mode)
  )
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

  /** Puts an existing account into the form; nothing secret is shown back. */
  function fillFrom(account: Account): void {
    editingAccountId.value = account.id
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

  return {
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
  }
}
