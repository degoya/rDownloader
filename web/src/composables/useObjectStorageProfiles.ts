import { ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import { trimmed } from '@/utils/values'
import type {
  CreateObjectStorageProfile,
  ObjectStorageAddressing,
  ObjectStorageCredentialSource,
  ObjectStorageProfile,
  ObjectStorageProvider,
  ObjectStorageTest,
  UpdateObjectStorageProfile
} from '@/api/types'

/**
 * Editable shape of the profile form. The secret and the session token are write-only: the
 * form never holds a stored value, and an empty field on an update keeps the stored one.
 */
export interface ObjectStorageForm {
  /** The driver field: it decides which of the others apply. */
  provider: ObjectStorageProvider
  credential_source: ObjectStorageCredentialSource
  name: string
  /** Empty means the provider's own service (AWS S3 for the region, the account's blob host, Google). */
  endpoint: string
  /** S3 only. */
  region: string
  /** Azure only. */
  account: string
  /** Empty means the profile is not bound to one bucket (container, on Azure). */
  bucket: string
  /** S3 only. `auto` follows the server's default: virtual-host without an endpoint, path with one. */
  addressing: ObjectStorageAddressing | 'auto'
  /** S3 only. */
  access_key_id: string
  /** The S3 secret key, the Azure account key or SAS, the Google service account key. */
  secret_access_key: string
  /** S3 only. */
  session_token: string
  clear_session_token: boolean
  /** S3 only. */
  checksums: boolean
  enabled: boolean
  /** The explicit yes an `ambient` profile with an endpoint needs to send the machine's credentials there. */
  ambient_custom_endpoint: boolean
}

export const PROVIDERS: readonly ObjectStorageProvider[] = ['s3', 'azure', 'gcs']

/** The link scheme of each provider, as `aws`, `az` and `gsutil` print it. */
const SCHEMES: Readonly<Record<ObjectStorageProvider, string>> = { s3: 's3', azure: 'az', gcs: 'gs' }

/** The credential sources a provider can sign with, the one a new profile starts with first. */
export function credentialSources(provider: ObjectStorageProvider): readonly ObjectStorageCredentialSource[] {
  return provider === 'azure'
    ? ['static', 'shared_access_signature', 'ambient', 'anonymous']
    : ['static', 'ambient', 'anonymous']
}

/** Whether the source signs with a secret the profile stores. */
export function storesSecret(source: ObjectStorageCredentialSource): boolean {
  return source === 'static' || source === 'shared_access_signature'
}

export function emptyForm(): ObjectStorageForm {
  return {
    provider: 's3',
    credential_source: 'static',
    name: '',
    endpoint: '',
    region: '',
    account: '',
    bucket: '',
    addressing: 'auto',
    access_key_id: '',
    secret_access_key: '',
    session_token: '',
    clear_session_token: false,
    checksums: true,
    enabled: true,
    ambient_custom_endpoint: false
  }
}

/** Fills the form from a stored profile. The secret fields stay blank by design. */
export function formFor(profile: ObjectStorageProfile): ObjectStorageForm {
  return {
    provider: profile.provider,
    credential_source: profile.credential_source,
    name: profile.name,
    endpoint: profile.endpoint ?? '',
    region: profile.region ?? '',
    account: profile.account ?? '',
    bucket: profile.bucket ?? '',
    addressing: profile.addressing,
    access_key_id: profile.access_key_id ?? '',
    secret_access_key: '',
    session_token: '',
    clear_session_token: false,
    checksums: profile.checksums,
    enabled: profile.enabled,
    ambient_custom_endpoint: profile.ambient_custom_endpoint
  }
}

/** Whether the form sends the machine's own credentials to an endpoint of its own. */
export function ambientAtEndpoint(form: ObjectStorageForm): boolean {
  return form.credential_source === 'ambient' && Boolean(form.endpoint.trim())
}

/**
 * The request for the form. Only what the provider and the source use travels: a key typed
 * before switching the source or the provider stays in the browser.
 */
export function toCreateBody(form: ObjectStorageForm): CreateObjectStorageProfile {
  const s3 = form.provider === 's3'
  const s3Keys = s3 && form.credential_source === 'static'
  return {
    name: form.name.trim(),
    provider: form.provider,
    endpoint: trimmed(form.endpoint),
    region: s3 ? trimmed(form.region) : null,
    account: form.provider === 'azure' ? trimmed(form.account) : null,
    bucket: trimmed(form.bucket),
    ...(!s3 || form.addressing === 'auto' ? {} : { addressing: form.addressing }),
    credential_source: form.credential_source,
    access_key_id: s3Keys ? trimmed(form.access_key_id) : null,
    secret_access_key: storesSecret(form.credential_source) ? trimmed(form.secret_access_key) : null,
    session_token: s3Keys ? trimmed(form.session_token) : null,
    checksums: s3 && form.checksums,
    enabled: form.enabled,
    ambient_custom_endpoint: ambientAtEndpoint(form) && form.ambient_custom_endpoint
  }
}

export function toUpdateBody(form: ObjectStorageForm): UpdateObjectStorageProfile {
  const s3Keys = form.provider === 's3' && form.credential_source === 'static'
  const clear = s3Keys && form.clear_session_token && !form.session_token.trim()
  return { ...toCreateBody(form), clear_session_token: clear }
}

/**
 * Whether the form still names the host a stored secret was typed for: the same endpoint, and
 * on Azure the same account. The server sends a secret to no other host and asks for it again.
 */
export function sameHost(form: ObjectStorageForm, stored: ObjectStorageProfile): boolean {
  const account = form.provider === 'azure' ? trimmed(form.account) : null
  return (stored.endpoint ?? null) === normalizedEndpoint(form.endpoint)
    && (stored.account ?? null) === account
}

/** The endpoint as the server stores it: trimmed, no trailing slash, empty as none. */
function normalizedEndpoint(value: string): string | null {
  const endpoint = trimmed(value)
  if (!endpoint) return null
  try {
    return new URL(endpoint).href.replace(/\/+$/, '')
  } catch {
    return endpoint
  }
}

/**
 * Whether the stored secret still signs for the form: the server keeps it only while the
 * provider, the source and the host stay what they were.
 */
export function keepsSecret(form: ObjectStorageForm, stored: ObjectStorageProfile | null): boolean {
  return Boolean(stored?.has_secret)
    && stored?.provider === form.provider
    && stored?.credential_source === form.credential_source
    && sameHost(form, stored)
}

/** A stored secret the form's new host drops: the page says so, and asks for it again. */
export function dropsSecretForHost(form: ObjectStorageForm, stored: ObjectStorageProfile | null): boolean {
  return Boolean(stored?.has_secret)
    && stored?.provider === form.provider
    && stored?.credential_source === form.credential_source
    && !sameHost(form, stored)
}

/**
 * Whether the form carries what the server's validation asks for, so the button does not offer
 * a request that fails. Azure needs its account; machine credentials at a custom endpoint need the
 * explicit yes; a new stored-key profile needs its secret (and on S3 the key id); an edit keeps a
 * stored secret, but a profile that had none, or had one for another provider, source or host,
 * needs one now.
 */
export function formComplete(form: ObjectStorageForm, stored: ObjectStorageProfile | null): boolean {
  if (!form.name.trim()) return false
  if (form.provider === 'azure' && !form.account.trim()) return false
  if (ambientAtEndpoint(form) && !form.ambient_custom_endpoint) return false
  if (!storesSecret(form.credential_source)) return true
  if (form.provider === 's3' && !form.access_key_id.trim()) return false
  return Boolean(form.secret_access_key.trim()) || keepsSecret(form, stored)
}

/** A stored-key profile without a stored secret (or S3 key id) cannot sign a request yet. */
export function isIncomplete(profile: ObjectStorageProfile): boolean {
  if (!storesSecret(profile.credential_source)) return false
  return !profile.has_secret || (profile.provider === 's3' && !profile.access_key_id)
}

/** The service a profile talks to: its endpoint's host, or the provider's own service. */
export function endpointLabel(profile: ObjectStorageProfile): string {
  if (profile.endpoint) {
    try {
      return new URL(profile.endpoint).host
    } catch {
      return profile.endpoint
    }
  }
  if (profile.provider === 'azure') return `${profile.account ?? ''}.blob.core.windows.net`
  if (profile.provider === 'gcs') return 'storage.googleapis.com'
  return profile.region ? `AWS S3 · ${profile.region}` : 'AWS S3'
}

/** The link to a profile's bound bucket, in its provider's scheme. */
export function bucketLink(profile: ObjectStorageProfile): string | null {
  return profile.bucket ? `${SCHEMES[profile.provider]}://${profile.bucket}` : null
}

/** The upload target string post-processing understands for this profile. */
export function uploadRemoteFor(profileId: string, prefix = ''): string {
  return `object-storage:${profileId}/${prefix.replace(/^\/+/, '')}`
}

/** The outcome of a test, reduced to what the card shows. */
type TestOutcome =
  | { ok: true }
  | { ok: false, code: string, params: Record<string, string> }

/**
 * A test failure is an answer, not an error: the service names it with a stable code. A result
 * without a code still says which of the two steps failed.
 */
export function testOutcome(result: ObjectStorageTest): TestOutcome {
  if (result.code) return { ok: false, code: result.code, params: result.params ?? {} }
  if (!result.reachable) return { ok: false, code: 'object_storage.connect_failed', params: {} }
  if (!result.authenticated) return { ok: false, code: 'object_storage.auth_failed', params: {} }
  return { ok: true }
}

/**
 * The enabled profiles, for a picker outside the card. A failed read answers an empty list, so
 * the picker stays hidden rather than offering targets it cannot vouch for.
 */
export async function enabledObjectStorageProfiles(): Promise<ObjectStorageProfile[]> {
  const response = await api.GET('/api/v1/object-storage/profiles')
  const profiles = Array.isArray(response.data) ? response.data : []
  return profiles.filter(profile => profile.enabled)
}

/**
 * State and calls of the object storage settings card. The card shows `error` and `message`
 * above its form, so nothing here raises a toast.
 */
export function useObjectStorageProfiles() {
  const profiles = ref<ObjectStorageProfile[]>([])
  const loading = ref(true)
  const pending = ref(false)
  const busyId = ref<string | null>(null)
  const error = ref<string | null>(null)
  const message = ref<string | null>(null)

  function clearFeedback(): void {
    error.value = null
    message.value = null
  }

  async function refresh(): Promise<void> {
    const response = await api.GET('/api/v1/object-storage/profiles')
    loading.value = false
    if (!response.data) return void (error.value = responseError(response))
    profiles.value = response.data
  }

  async function create(form: ObjectStorageForm): Promise<ObjectStorageProfile | null> {
    clearFeedback()
    pending.value = true
    const response = await api.POST('/api/v1/object-storage/profiles', { body: toCreateBody(form) })
    pending.value = false
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    const saved = response.data
    profiles.value = [...profiles.value, saved]
    return saved
  }

  async function update(id: string, form: ObjectStorageForm): Promise<ObjectStorageProfile | null> {
    clearFeedback()
    pending.value = true
    const response = await api.PUT('/api/v1/object-storage/profiles/{id}', { params: { path: { id } }, body: toUpdateBody(form) })
    pending.value = false
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    const saved = response.data
    profiles.value = profiles.value.map(existing => (existing.id === id ? saved : existing))
    return saved
  }

  /** Judged by the absence of an error, like every delete in the application. */
  async function remove(id: string): Promise<boolean> {
    clearFeedback()
    busyId.value = id
    const response = await api.DELETE('/api/v1/object-storage/profiles/{id}', { params: { path: { id } } })
    busyId.value = null
    if (response.error !== undefined) {
      error.value = responseError(response)
      return false
    }
    profiles.value = profiles.value.filter(profile => profile.id !== id)
    message.value = resultMessage(response.data)
    return true
  }

  /** Runs the live check; `null` when the request itself failed (the error is set then). */
  async function test(id: string): Promise<TestOutcome | null> {
    clearFeedback()
    busyId.value = id
    const response = await api.POST('/api/v1/object-storage/profiles/{id}/test', { params: { path: { id } } })
    busyId.value = null
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    return testOutcome(response.data)
  }

  return { profiles, loading, pending, busyId, error, message, clearFeedback, refresh, create, update, remove, test }
}
