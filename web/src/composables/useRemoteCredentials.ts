import { computed, ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type {
  CreateRemoteCredential,
  RemoteAuthMode,
  RemoteCredential,
  RemoteProtocol,
  SshHostKey,
  UpdateRemoteCredential
} from '@/api/types'

/** Editable shape of the login form; credentials are write-only and never read back. */
export interface RemoteCredentialForm {
  name: string
  protocol: RemoteProtocol
  host: string
  /** Empty means "use the protocol default", which the server fills in. */
  port: string
  username: string
  auth_mode: RemoteAuthMode
  passive: boolean
  secret: string
  private_key: string
  passphrase: string
  enabled: boolean
}

/** Default port per protocol, mirroring `RemoteProtocol::default_port`. */
export const DEFAULT_PORTS: Record<RemoteProtocol, number> = {
  ftp: 21,
  ftps: 21,
  ftps_implicit: 990,
  sftp: 22,
  webdav: 443
}

/** Which authentication methods each protocol accepts, mirroring `is_valid_for`. */
export function authModesFor(protocol: RemoteProtocol): RemoteAuthMode[] {
  // Keys and agents are SSH concepts; FTP has no equivalent.
  return protocol === 'sftp'
    ? ['password', 'private_key', 'agent']
    : ['anonymous', 'password']
}

export function emptyForm(): RemoteCredentialForm {
  return {
    name: '',
    protocol: 'ftp',
    host: '',
    port: '',
    username: '',
    auth_mode: 'password',
    passive: true,
    secret: '',
    private_key: '',
    passphrase: '',
    enabled: true
  }
}

/** Fills the form from a stored login. Credential fields stay blank by design. */
export function formFor(credential: RemoteCredential): RemoteCredentialForm {
  return {
    name: credential.name,
    protocol: credential.protocol,
    host: credential.host,
    // Only show a port that differs from the default, so the field reads as "default".
    port: credential.port === DEFAULT_PORTS[credential.protocol] ? '' : String(credential.port),
    username: credential.username ?? '',
    auth_mode: credential.auth_mode,
    passive: credential.passive ?? true,
    secret: '',
    private_key: '',
    passphrase: '',
    enabled: credential.enabled
  }
}

function trimmed(value: string): string | null {
  const text = value.trim()
  return text.length > 0 ? text : null
}

function portOf(form: RemoteCredentialForm): number | null {
  const text = form.port.trim()
  if (!text) return null
  const parsed = Number.parseInt(text, 10)
  return Number.isInteger(parsed) && parsed > 0 && parsed <= 65535 ? parsed : null
}

export function toCreateBody(form: RemoteCredentialForm): CreateRemoteCredential {
  const usesPassword = form.auth_mode === 'password'
  const usesKey = form.auth_mode === 'private_key'
  return {
    name: form.name.trim(),
    protocol: form.protocol,
    host: form.host.trim(),
    port: portOf(form),
    // Anonymous FTP has a fixed user name the server supplies.
    username: form.auth_mode === 'anonymous' ? null : trimmed(form.username),
    auth_mode: form.auth_mode,
    passive: form.passive,
    secret: usesPassword ? trimmed(form.secret) : null,
    private_key: usesKey ? trimmed(form.private_key) : null,
    passphrase: usesKey ? trimmed(form.passphrase) : null,
    enabled: form.enabled
  }
}

export function toUpdateBody(
  form: RemoteCredentialForm,
  clearPrivateKey: boolean
): UpdateRemoteCredential {
  return { ...toCreateBody(form), clear_private_key: clearPrivateKey }
}

/** Human-readable endpoint, matching how a link is matched against it. */
export function endpointLabel(credential: RemoteCredential): string {
  const port = credential.port === DEFAULT_PORTS[credential.protocol] ? '' : `:${credential.port}`
  const user = credential.username ? `${credential.username}@` : ''
  return `${user}${credential.host}${port}`
}

/** Stable identity of one trusted host key, used as a list key and for removal. */
export function hostKeyId(key: SshHostKey): string {
  return `${key.host}:${key.port}:${key.algorithm}`
}

/** CRUD state for the remote logins settings card. */
export function useRemoteCredentials(emit: (event: 'message' | 'error', text: string) => void) {
  const credentials = ref<RemoteCredential[]>([])
  const hostKeys = ref<SshHostKey[]>([])
  const loading = ref(true)
  const pending = ref(false)
  const busyId = ref<string | null>(null)

  /** Logins that cannot be used until something is supplied or fixed. */
  const incomplete = computed(() =>
    credentials.value.filter(
      credential =>
        (credential.auth_mode === 'password' && !credential.has_secret) ||
        (credential.auth_mode === 'private_key' && !credential.has_key)
    )
  )

  async function refresh(): Promise<void> {
    const [list, keys] = await Promise.all([
      api.GET('/api/v1/remote-credentials'),
      api.GET('/api/v1/remote-credentials/ssh-hosts')
    ])
    loading.value = false
    if (!list.data) return void emit('error', responseError(list))
    credentials.value = list.data
    hostKeys.value = keys.data ?? []
  }

  async function create(form: RemoteCredentialForm): Promise<boolean> {
    pending.value = true
    const response = await api.POST('/api/v1/remote-credentials', { body: toCreateBody(form) })
    pending.value = false
    if (!response.data) {
      emit('error', responseError(response))
      return false
    }
    credentials.value = [...credentials.value, response.data]
    return true
  }

  async function update(
    id: string,
    form: RemoteCredentialForm,
    clearPrivateKey = false
  ): Promise<boolean> {
    pending.value = true
    const response = await api.PUT('/api/v1/remote-credentials/{id}', {
      params: { path: { id } },
      body: toUpdateBody(form, clearPrivateKey)
    })
    pending.value = false
    if (!response.data) {
      emit('error', responseError(response))
      return false
    }
    credentials.value = credentials.value.map(existing =>
      existing.id === id ? response.data : existing
    )
    return true
  }

  /**
   * Runs the live check. A failure is an answer rather than an error, and the most common
   * one — an unconfirmed SSH host key — carries the fingerprint the user then confirms, so
   * the caller gets the code and parameters rather than a flattened string.
   */
  async function test(id: string): Promise<{ code: string; params: Record<string, string> } | null> {
    busyId.value = id
    const response = await api.POST('/api/v1/remote-credentials/{id}/test', {
      params: { path: { id } }
    })
    busyId.value = null
    if (!response.data) {
      emit('error', responseError(response))
      return null
    }
    if (response.data.authenticated) {
      emit('message', 'ok')
      return null
    }
    return { code: response.data.code ?? '', params: response.data.params ?? {} }
  }

  async function remove(id: string): Promise<void> {
    busyId.value = id
    const response = await api.DELETE('/api/v1/remote-credentials/{id}', {
      params: { path: { id } }
    })
    busyId.value = null
    if (!response.data) return void emit('error', responseError(response))
    credentials.value = credentials.value.filter(credential => credential.id !== id)
    emit('message', resultMessage(response.data))
  }

  /** Confirms one host key. The fingerprint is required, never inferred. */
  async function trustHostKey(key: {
    host: string
    port: number
    algorithm: string
    fingerprint: string
  }): Promise<boolean> {
    pending.value = true
    const response = await api.POST('/api/v1/remote-credentials/ssh-hosts', { body: key })
    pending.value = false
    if (!response.data) {
      emit('error', responseError(response))
      return false
    }
    emit('message', resultMessage(response.data))
    await refresh()
    return true
  }

  async function forgetHostKey(key: SshHostKey): Promise<void> {
    busyId.value = hostKeyId(key)
    const response = await api.DELETE(
      '/api/v1/remote-credentials/ssh-hosts/{host}/{port}/{algorithm}',
      { params: { path: { host: key.host, port: key.port, algorithm: key.algorithm } } }
    )
    busyId.value = null
    if (!response.data) return void emit('error', responseError(response))
    hostKeys.value = hostKeys.value.filter(existing => hostKeyId(existing) !== hostKeyId(key))
    emit('message', resultMessage(response.data))
  }

  return {
    credentials,
    hostKeys,
    loading,
    pending,
    busyId,
    incomplete,
    refresh,
    create,
    update,
    test,
    remove,
    trustHostKey,
    forgetHostKey
  }
}
