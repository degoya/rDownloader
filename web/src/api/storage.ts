/**
 * Collision policies and prompts, duplicates, dedupe links and the storage history
 * (RD-150-01, RD-150-02).
 *
 * Plain `fetch` like the plugin repository module beside it: every refusal has to keep its coded
 * message, because a refused link ("the files differ", "in use") is the one thing the person has
 * to read before trying anything else.
 */
import { withBase } from '@/basePath'
import { serverMessageFrom, type ServerMessage } from '@/i18n/server'

import type { components } from './schema'

type Schemas = components['schemas']

export type CollisionPolicy = Schemas['CollisionPolicy']
export type CollisionDecision = Schemas['CollisionDecision']
export type CollisionPolicySource = Schemas['CollisionPolicySource']

export const COLLISION_POLICIES: readonly CollisionPolicy[] = ['rename', 'skip', 'overwrite', 'compare', 'ask']
export const COLLISION_DECISIONS: readonly CollisionDecision[] = ['rename', 'skip', 'overwrite']

export type CollisionPolicies = Schemas['CollisionPoliciesResponse']
export type PackageCollisionPolicy = Schemas['PackageCollisionPolicyResponse']
export type CollisionPrompt = Schemas['CollisionPromptResponse']
export type SourceIdentity = Schemas['SourceIdentity']
export type SourceDuplicate = Schemas['SourceDuplicate']
export type ContentDuplicate = Schemas['ContentDuplicate']
export type DuplicateReport = Schemas['DuplicateReport']
export type DuplicateLookupEntry = Schemas['DuplicateLookupEntry']
export type ReuseCapability = Schemas['ReuseCapability']
export type RunnerReuse = Schemas['RunnerReuseResponse']
export type LinkSupport = Schemas['LinkSupportEntry']
export type StorageOperation = Schemas['StorageOperationResponse']
export type ContentIndexCheck = Schemas['ContentIndexCheckResponse']
export type DedupeResult = Schemas['DedupeResponse']

/** The body of a successful answer, or the coded message of a refusal (`null` without one). */
export type Answer<T> =
  | { ok: true, data: T }
  | { ok: false, status: number, message: ServerMessage | null }

async function call<T>(method: string, path: string, body?: object): Promise<Answer<T>> {
  const init: RequestInit = { method, credentials: 'same-origin' }
  if (body !== undefined) {
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

export const listCollisionPolicies = (): Promise<Answer<CollisionPolicies>> =>
  call('GET', '/api/v1/collision-policies')

export const setCategoryCollisionPolicy = (categoryId: string, policy: CollisionPolicy | null): Promise<Answer<ServerMessage>> =>
  call('PUT', `/api/v1/categories/${encodeURIComponent(categoryId)}/collision-policy`, { policy })

export const getPackageCollisionPolicy = (packageId: string): Promise<Answer<PackageCollisionPolicy>> =>
  call('GET', `/api/v1/packages/${encodeURIComponent(packageId)}/collision-policy`)

export const setPackageCollisionPolicy = (packageId: string, policy: CollisionPolicy | null): Promise<Answer<PackageCollisionPolicy>> =>
  call('PUT', `/api/v1/packages/${encodeURIComponent(packageId)}/collision-policy`, { policy })

export const listCollisionPrompts = (): Promise<Answer<CollisionPrompt[]>> =>
  call('GET', '/api/v1/collision-prompts')

export const decideCollision = (downloadId: string, decision: CollisionDecision): Promise<Answer<ServerMessage>> =>
  call('POST', `/api/v1/downloads/${encodeURIComponent(downloadId)}/collision-decision`, { decision })

export const getDownloadDuplicates = (downloadId: string): Promise<Answer<DuplicateReport>> =>
  call('GET', `/api/v1/downloads/${encodeURIComponent(downloadId)}/duplicates`)

export const lookupDuplicates = (urls: string[]): Promise<Answer<DuplicateLookupEntry[]>> =>
  call('POST', '/api/v1/duplicates/lookup', { urls })

export const dedupeDownload = (downloadId: string, originalDownloadId: string): Promise<Answer<DedupeResult>> =>
  call('POST', `/api/v1/downloads/${encodeURIComponent(downloadId)}/dedupe`, {
    original_download_id: originalDownloadId,
    mode: 'hardlink'
  })

export const listReuseCapabilities = (): Promise<Answer<RunnerReuse[]>> =>
  call('GET', '/api/v1/storage/reuse')

export const listLinkSupport = (): Promise<Answer<LinkSupport[]>> =>
  call('GET', '/api/v1/storage/link-support')

export const listStorageOperations = (limit = 50): Promise<Answer<StorageOperation[]>> =>
  call('GET', `/api/v1/storage/operations?limit=${limit}`)

export const checkContentIndex = (): Promise<Answer<ContentIndexCheck>> =>
  call('POST', '/api/v1/storage/content-index/check', {})
