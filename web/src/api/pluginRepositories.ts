/**
 * The plugin repository routes and the install preview (RD-140-01).
 *
 * Addressed by hand through the coded `call`, for two reasons: the preview and the upload
 * install send the `.rdplug` bytes as the body, which the generated client does not model, and
 * every answer has to keep its coded message — a `409` that asks for a key to be confirmed
 * carries the fingerprint in its parameters, and that is the one thing the person has to see
 * before anything is trusted.
 *
 * The shapes are the generated schema's (WEB-04): written out by hand they would drift from the
 * service without `vue-tsc` noticing. `Refine` only puts back the closed sets the schema writes
 * as a bare `string`.
 */
import { call, type Answer, type CallPath } from './call'
import type { components } from './schema'

type Schemas = components['schemas']

/** A schema shape with some of its fields narrowed to what the service actually sends. */
export type Refine<T, R> = Omit<T, keyof R> & R

export type PluginRepository = Refine<Schemas['PluginRepositoryResponse'], { kind: 'official' | 'third_party' }>
export type PluginRepositories = Refine<Schemas['PluginRepositoriesResponse'], { repositories: PluginRepository[] }>
export type PluginPermissions = Schemas['PluginPermissionsResponse']

type PluginCompatibility = 'compatible' | 'contract_unsupported' | 'app_too_old' | 'withdrawn'

export type PluginOffer = Refine<Schemas['PluginOfferResponse'], { compatibility: PluginCompatibility }>

/**
 * `adds_permissions`: asks for a permission the installed version lacks and waits for a click
 * whatever the policy; `added_permissions` lists them, empty when it does not (RD-160-09).
 */
export type PluginUpdate = Refine<Schemas['PluginUpdateResponse'], { offer: PluginOffer, policy: Schemas['PluginUpdatePolicy'] }>

/** `installed`: every offered version of a plugin installed here, for the release-notes history. */
export type PluginOffers = Refine<Schemas['PluginOffersResponse'], {
  updates: PluginUpdate[]
  available: PluginOffer[]
  installed: PluginOffer[]
}>

type PluginKeyStatus = 'trusted' | 'untrusted' | 'mismatch' | 'withdrawn' | 'unsigned'

/**
 * `added_permissions`: what the package asks for beyond the newest installed version
 * (RD-160-09); `null` when no version of the plugin is installed, so every permission is new.
 */
export type PluginPreview = Refine<Schemas['PluginPreviewResponse'], { key_status: PluginKeyStatus }>

/** Where a previewed package comes from: a file the person picked, or a repository's offer. */
export type PreviewSource =
  | { kind: 'upload', file: Blob }
  | { kind: 'repository', repositoryId: string, pluginId: string, version: string }

function query(trustFingerprint?: string): '' | `?${string}` {
  return trustFingerprint ? `?trust_fingerprint=${encodeURIComponent(trustFingerprint)}` : ''
}

function repositoryPath(id: string, action: '' | '/preview' | '/install' = ''): CallPath {
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

/** Whether every installed plugin installs its updates itself (RD-191-10). */
export type PluginUpdateSettings = Schemas['PluginUpdateSettingsResponse']

export const getUpdateSettings = () =>
  call<PluginUpdateSettings>('GET', '/api/v1/plugins/updates/settings')

export const setUpdateSettings = (automatic: boolean) =>
  call<PluginUpdateSettings>('PUT', '/api/v1/plugins/updates/settings', { automatic_updates: automatic })

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
