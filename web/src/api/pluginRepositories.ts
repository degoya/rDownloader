/**
 * The plugin repository routes and the install preview (RD-140-01).
 *
 * Plain `fetch` like the package upload beside them, for two reasons: the preview and the
 * upload install send the `.rdplug` bytes as the body, which the generated client does not
 * model, and every answer has to keep its coded message — a `409` that asks for a key to be
 * confirmed carries the fingerprint in its parameters, and that is the one thing the person has
 * to see before anything is trusted.
 */
import { withBase } from '@/basePath'
import { serverMessageFrom, type ServerMessage } from '@/i18n/server'

export interface PluginRepository {
  id: string
  kind: 'official' | 'third_party'
  name: string
  url: string
  key_id: string | null
  fingerprint: string | null
  enabled: boolean
  sequence: number | null
  issued_at: string | null
  expires_at: string | null
  last_checked_at: string | null
  last_success_at: string | null
  /** Stable code of the last check's failure. */
  last_error: string | null
}

export interface PluginRepositories {
  repositories: PluginRepository[]
  refresh_hours: number
}

export interface PluginPublisher {
  key_id: string
  fingerprint: string
  author: string
}

export interface PluginPermissions {
  granted: string[]
  http_domains: string[]
  stream_hosts: string[]
}

export interface PluginIndexPackage {
  plugin_id: string
  name: string
  version: string
  plugin_type: string
  api_version: string
  min_app_version: string | null
  package_digest: string
  size: number
  publisher: PluginPublisher
  permissions: PluginPermissions
  release_notes: string | null
}

export type PluginCompatibility = 'compatible' | 'contract_unsupported' | 'app_too_old' | 'withdrawn'

export interface PluginOffer {
  repository_id: string
  repository_name: string
  official: boolean
  package: PluginIndexPackage
  compatibility: PluginCompatibility
  installed_version: string | null
}

export interface PluginUpdate {
  offer: PluginOffer
  installed_version: string
  policy: 'manual' | 'automatic'
  /** Asks for a permission the installed version lacks; waits for a click whatever the policy. */
  adds_permissions: boolean
}

export interface PluginOffers {
  updates: PluginUpdate[]
  available: PluginOffer[]
  /** Every offered version of a plugin installed here, for the release-notes history. */
  installed?: PluginOffer[]
}

export type PluginKeyStatus = 'trusted' | 'untrusted' | 'mismatch' | 'withdrawn' | 'unsigned'

export interface PluginPreview {
  plugin_id: string
  name: string
  version: string
  plugin_type: string
  api_version: string
  min_app_version: string | null
  description: string
  homepage: string | null
  license: string | null
  package_digest: string
  size: number
  publisher: PluginPublisher | null
  permissions: PluginPermissions
  key_status: PluginKeyStatus
  withdrawn: boolean
  incompatible: string | null
  installable: boolean
  installed_versions: string[]
  source: { repository_id: string, repository_name: string, official: boolean } | null
  release_notes: string | null
}

/** Where a previewed package comes from: a file the person picked, or a repository's offer. */
export type PreviewSource =
  | { kind: 'upload', file: Blob }
  | { kind: 'repository', repositoryId: string, pluginId: string, version: string }

/** The body of a successful answer, or the coded message of a refusal (`null` without one). */
export type Answer<T> =
  | { ok: true, data: T }
  | { ok: false, status: number, message: ServerMessage | null }

async function call<T>(method: string, path: string, body?: Blob | object): Promise<Answer<T>> {
  const init: RequestInit = { method, credentials: 'same-origin' }
  if (body instanceof Blob) {
    init.headers = { 'Content-Type': 'application/octet-stream' }
    init.body = body
  } else if (body !== undefined) {
    init.headers = { 'Content-Type': 'application/json' }
    init.body = JSON.stringify(body)
  }
  try {
    const response = await fetch(withBase(path), init)
    const payload: unknown = await response.json().catch(() => null)
    if (response.ok) return { ok: true, data: payload as T }
    return { ok: false, status: response.status, message: serverMessageFrom(payload) }
  } catch {
    return { ok: false, status: 0, message: null }
  }
}

function query(trustFingerprint?: string): string {
  return trustFingerprint ? `?trust_fingerprint=${encodeURIComponent(trustFingerprint)}` : ''
}

function repositoryPath(id: string, action = ''): string {
  return `/api/v1/plugins/repositories/${encodeURIComponent(id)}${action}`
}

export const listRepositories = () =>
  call<PluginRepositories>('GET', '/api/v1/plugins/repositories')

export const addRepository = (
  input: { url: string, public_key: string, name?: string },
  trustFingerprint?: string
) => call<PluginRepository>('POST', `/api/v1/plugins/repositories${query(trustFingerprint)}`, input)

export const updateRepository = (id: string, patch: { enabled?: boolean, name?: string }) =>
  call<unknown>('PATCH', repositoryPath(id), patch)

export const removeRepository = (id: string) => call<unknown>('DELETE', repositoryPath(id))

export const refreshRepositories = () =>
  call<PluginRepositories>('POST', '/api/v1/plugins/repositories/refresh', {})

export const setRefreshHours = (hours: number) =>
  call<PluginRepositories>('PUT', '/api/v1/plugins/repositories/settings', { refresh_hours: hours })

export const listOffers = () => call<PluginOffers>('GET', '/api/v1/plugins/updates')

/** Release notes a repository index delivered for one version of a plugin. */
export interface ReleaseNote {
  version: string
  notes: string
  repository: string
}

function versionParts(version: string): number[] {
  return version.split(/[.+-]/).map(part => Number.parseInt(part, 10)).map(part => (Number.isNaN(part) ? 0 : part))
}

/** Newest first, by the numeric parts of the version. */
function compareVersionsDesc(left: string, right: string): number {
  const a = versionParts(left)
  const b = versionParts(right)
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (b[index] ?? 0) - (a[index] ?? 0)
    if (difference !== 0) return difference
  }
  return 0
}

/**
 * The release notes of every offered version, by plugin id, newest first (RD-140-02).
 *
 * Only versions that carry notes appear, so a plugin whose index says nothing gets no entry and
 * the version panel renders no notes section for it. The first repository listed wins a
 * version two repositories offer, which is the official one — the order the service lists them.
 */
export function releaseNotesByPlugin(offers: PluginOffers): Map<string, ReleaseNote[]> {
  const byPlugin = new Map<string, ReleaseNote[]>()
  // Read as defensively as the updates list reads the same answer.
  const updates = Array.isArray(offers?.updates) ? offers.updates : []
  const available = Array.isArray(offers?.available) ? offers.available : []
  const installed = Array.isArray(offers?.installed) ? offers.installed : []
  for (const offer of [...updates.map(update => update.offer), ...installed, ...available]) {
    const notes = offer.package.release_notes?.trim()
    if (!notes) continue
    const list = byPlugin.get(offer.package.plugin_id) ?? []
    if (list.some(entry => entry.version === offer.package.version)) continue
    list.push({ version: offer.package.version, notes, repository: offer.repository_name })
    byPlugin.set(offer.package.plugin_id, list)
  }
  for (const list of byPlugin.values()) list.sort((left, right) => compareVersionsDesc(left.version, right.version))
  return byPlugin
}

export function preview(source: PreviewSource): Promise<Answer<PluginPreview>> {
  if (source.kind === 'upload') return call('POST', '/api/v1/plugins/preview', source.file)
  return call('POST', repositoryPath(source.repositoryId, '/preview'), {
    plugin_id: source.pluginId,
    version: source.version
  })
}

export function install(source: PreviewSource, trustFingerprint?: string): Promise<Answer<unknown>> {
  if (source.kind === 'upload') {
    return call('POST', `/api/v1/plugins/install${query(trustFingerprint)}`, source.file)
  }
  return call('POST', repositoryPath(source.repositoryId, '/install'), {
    plugin_id: source.pluginId,
    version: source.version,
    ...(trustFingerprint ? { trust_fingerprint: trustFingerprint } : {})
  })
}

/** Splits a hex fingerprint into 8-character blocks so it can be compared by eye. */
export function groupFingerprint(fingerprint: string): string {
  return (fingerprint.match(/.{1,8}/g) ?? [fingerprint]).join(' ')
}
