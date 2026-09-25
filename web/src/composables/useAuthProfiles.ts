import { computed, ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type {
  AuthMethod,
  AuthProfile,
  AuthProfileSelection,
  CreateAuthProfile,
  UpdateAuthProfile
} from '@/api/types'

/** Editable shape of the profile form; credentials are write-only and never read back. */
export interface AuthProfileForm {
  name: string
  scope: string
  include_subdomains: boolean
  method: AuthMethod
  username: string
  secret: string
  certificate_pem: string
  expires_at: string
  enabled: boolean
}

export function emptyForm(): AuthProfileForm {
  return {
    name: '',
    scope: '',
    include_subdomains: true,
    method: 'cookies',
    username: '',
    secret: '',
    certificate_pem: '',
    expires_at: '',
    enabled: true
  }
}

/** Fills the form from a stored profile. Credential fields stay blank by design. */
export function formFor(profile: AuthProfile): AuthProfileForm {
  return {
    name: profile.name,
    scope: profile.path_prefix ? `${profile.host}${profile.path_prefix}` : profile.host,
    include_subdomains: profile.include_subdomains,
    method: profile.method,
    username: profile.username ?? '',
    secret: '',
    certificate_pem: '',
    expires_at: profile.expires_at?.slice(0, 10) ?? '',
    enabled: profile.enabled
  }
}

function trimmed(value: string): string | null {
  const text = value.trim()
  return text.length > 0 ? text : null
}

/** Local dates become an end-of-day UTC instant, matching what the user means by "until". */
function expiryAt(value: string): string | null {
  const text = value.trim()
  if (!text) return null
  const parsed = new Date(`${text}T23:59:59Z`)
  return Number.isNaN(parsed.getTime()) ? null : parsed.toISOString()
}

export function toCreateBody(form: AuthProfileForm): CreateAuthProfile {
  return {
    name: form.name.trim(),
    scope: form.scope.trim(),
    include_subdomains: form.include_subdomains,
    method: form.method,
    username: form.method === 'basic' ? trimmed(form.username) : null,
    secret: trimmed(form.secret),
    certificate_pem: trimmed(form.certificate_pem),
    expires_at: expiryAt(form.expires_at),
    enabled: form.enabled
  }
}

export function toUpdateBody(form: AuthProfileForm, clearCertificate: boolean): UpdateAuthProfile {
  return { ...toCreateBody(form), clear_certificate: clearCertificate }
}

/** A profile whose expiry has passed is still listed, but no longer applied by the server. */
export function isExpired(profile: AuthProfile, now: Date = new Date()): boolean {
  return profile.expires_at != null && new Date(profile.expires_at).getTime() <= now.getTime()
}

/** Only an enabled, unexpired profile is actually used for a download. */
export function isUsable(profile: AuthProfile, now: Date = new Date()): boolean {
  return profile.enabled && !isExpired(profile, now)
}

/** Human-readable scope, matching how the server matches it. */
export function scopeLabel(profile: AuthProfile): string {
  const host = profile.include_subdomains ? `*.${profile.host}` : profile.host
  return `${host}${profile.path_prefix ?? ''}`
}

/**
 * Ranks profiles the way the server picks one: more host labels first, then the longer
 * path prefix. Used to show which profile would win for a given host.
 */
function specificity(profile: AuthProfile): [number, number] {
  return [profile.host.split('.').length, profile.path_prefix?.length ?? 0]
}

function hostMatches(profile: AuthProfile, host: string): boolean {
  const target = host.replace(/\.$/, '').toLowerCase()
  if (target === profile.host) return true
  return profile.include_subdomains && target.endsWith(`.${profile.host}`)
}

function pathMatches(profile: AuthProfile, path: string): boolean {
  const prefix = (profile.path_prefix ?? '').replace(/\/+$/, '')
  if (!prefix) return true
  if (!path.startsWith(prefix)) return false
  const rest = path.slice(prefix.length)
  return rest === '' || rest.startsWith('/')
}

/**
 * The profile the server would apply to `url`, or null. Mirrors `AuthScope::matches_url`
 * plus the specificity ranking so the UI can preview the choice without a round trip.
 */
export function matchFor(profiles: AuthProfile[], url: string, now: Date = new Date()): AuthProfile | null {
  let parsed: URL
  try {
    parsed = new URL(/^[a-z]+:\/\//i.test(url) ? url : `https://${url}`)
  } catch {
    return null
  }
  const host = parsed.hostname
  const candidates = profiles.filter(
    profile => isUsable(profile, now) && hostMatches(profile, host) && pathMatches(profile, parsed.pathname)
  )
  return candidates.reduce<AuthProfile | null>((best, profile) => {
    if (!best) return profile
    const [labels, path] = specificity(profile)
    const [bestLabels, bestPath] = specificity(best)
    if (labels !== bestLabels) return labels > bestLabels ? profile : best
    if (path !== bestPath) return path > bestPath ? profile : best
    return best
  }, null)
}

/**
 * CRUD state for the auth profile settings card. Kept out of the component so the rules
 * above stay testable on their own.
 */
export function useAuthProfiles(emit: (event: 'message' | 'error', text: string) => void) {
  const profiles = ref<AuthProfile[]>([])
  const loading = ref(true)
  const pending = ref(false)
  const busyId = ref<string | null>(null)
  /**
   * Why the last save was refused, shown at the form rather than at the foot of the page: a
   * refused cookie row (RD-120-54) is only fixable while its text is still in view.
   */
  const formError = ref<string | null>(null)

  /** Captured sessions arrive disabled and need an explicit approval click. */
  const awaitingApproval = computed(() =>
    profiles.value.filter(profile => profile.origin === 'browser_capture' && !profile.enabled)
  )

  async function refresh(): Promise<void> {
    const response = await api.GET('/api/v1/auth-profiles')
    loading.value = false
    if (!response.data) return void emit('error', responseError(response))
    profiles.value = response.data
  }

  async function create(form: AuthProfileForm): Promise<boolean> {
    pending.value = true
    formError.value = null
    const response = await api.POST('/api/v1/auth-profiles', { body: toCreateBody(form) })
    pending.value = false
    if (!response.data) {
      formError.value = responseError(response)
      return false
    }
    profiles.value = [...profiles.value, response.data]
    return true
  }

  async function update(id: string, form: AuthProfileForm, clearCertificate = false): Promise<boolean> {
    pending.value = true
    formError.value = null
    const response = await api.PUT('/api/v1/auth-profiles/{id}', {
      params: { path: { id } },
      body: toUpdateBody(form, clearCertificate)
    })
    pending.value = false
    if (!response.data) {
      formError.value = responseError(response)
      return false
    }
    replace(response.data)
    return true
  }

  async function setEnabled(id: string, enabled: boolean): Promise<void> {
    busyId.value = id
    const path = enabled ? '/api/v1/auth-profiles/{id}/enable' : '/api/v1/auth-profiles/{id}/disable'
    const response = await api.POST(path, { params: { path: { id } } })
    busyId.value = null
    if (!response.data) return void emit('error', responseError(response))
    replace(response.data)
  }

  async function test(id: string): Promise<void> {
    busyId.value = id
    const response = await api.POST('/api/v1/auth-profiles/{id}/test', { params: { path: { id } } })
    busyId.value = null
    if (!response.data) return void emit('error', responseError(response))
    // Reachable but unauthenticated is a real answer, not an error: the scope responded,
    // it just did not accept the credential.
    emit(response.data.authenticated ? 'message' : 'error', response.data.authenticated
      ? String(response.data.status ?? 200)
      : String(response.data.status ?? ''))
  }

  async function remove(id: string): Promise<void> {
    busyId.value = id
    const response = await api.DELETE('/api/v1/auth-profiles/{id}', { params: { path: { id } } })
    busyId.value = null
    if (!response.data) return void emit('error', responseError(response))
    profiles.value = profiles.value.filter(profile => profile.id !== id)
    emit('message', resultMessage(response.data))
  }

  function replace(profile: AuthProfile): void {
    profiles.value = profiles.value.map(existing => (existing.id === profile.id ? profile : existing))
  }

  return { profiles, loading, pending, busyId, formError, awaitingApproval, refresh, create, update, setEnabled, test, remove }
}

/** Shared, lazily loaded profile list for the per-job selector. */
const options = ref<AuthProfile[]>([])
let optionsLoaded = false

async function loadOptions(): Promise<void> {
  const response = await api.GET('/api/v1/auth-profiles')
  options.value = response.data ?? []
}

/**
 * The per-job auth profile selector: "auto" lets the scope decide, "none" deliberately
 * sends nothing, and a profile id pins one. Expired profiles are offered but flagged,
 * because the server refuses a pinned profile that has expired.
 */
export function useAuthProfileSelector() {
  if (!optionsLoaded) {
    optionsLoaded = true
    void loadOptions()
  }

  const usable = computed(() => options.value.filter(profile => isUsable(profile)))

  /** Serialises the selector's flat value into the API's tagged selection. */
  function toSelection(value: string): AuthProfileSelection {
    if (value === 'auto' || value === 'none') return { mode: value } as AuthProfileSelection
    return { mode: 'pinned', id: value } as AuthProfileSelection
  }

  /** Flattens the API's tagged selection for a plain select. */
  function fromSelection(selection: AuthProfileSelection | null | undefined): string {
    if (!selection) return 'auto'
    return selection.mode === 'pinned' ? (selection.id ?? 'auto') : selection.mode
  }

  async function assign(downloadId: string, value: string): Promise<boolean> {
    const response = await api.PUT('/api/v1/downloads/{id}/auth-profile', {
      params: { path: { id: downloadId } },
      body: { auth_profile: toSelection(value) }
    })
    return response.data != null
  }

  return { options, usable, toSelection, fromSelection, assign, refresh: loadOptions }
}
