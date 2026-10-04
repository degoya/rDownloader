/**
 * Collision policies and prompts, duplicates, dedupe links and the storage history
 * (RD-150-01, RD-150-02).
 *
 * Through the coded `call` like the plugin repository module beside it: every refusal has to keep
 * its coded message, because a refused link ("the files differ", "in use") is the one thing the
 * person has to read before trying anything else.
 */
import type { ServerMessage } from '@/i18n/server'

import { call, type Answer } from './call'
import type { components } from './schema'

type Schemas = components['schemas']

export type CollisionPolicy = Schemas['CollisionPolicy']
export type CollisionDecision = Schemas['CollisionDecision']
type CollisionPolicySource = Schemas['CollisionPolicySource']

export const COLLISION_POLICIES: readonly CollisionPolicy[] = ['rename', 'skip', 'overwrite', 'compare', 'ask']
export const COLLISION_DECISIONS: readonly CollisionDecision[] = ['rename', 'skip', 'overwrite']

type CollisionPolicies = Schemas['CollisionPoliciesResponse']
export type PackageCollisionPolicy = Schemas['PackageCollisionPolicyResponse']
export type CollisionPrompt = Schemas['CollisionPromptResponse']
type SourceIdentity = Schemas['SourceIdentity']
type SourceDuplicate = Schemas['SourceDuplicate']
export type ContentDuplicate = Schemas['ContentDuplicate']
export type DuplicateReport = Schemas['DuplicateReport']
type DuplicateLookupEntry = Schemas['DuplicateLookupEntry']
export type ReuseCapability = Schemas['ReuseCapability']
export type RunnerReuse = Schemas['RunnerReuseResponse']
export type LinkSupport = Schemas['LinkSupportEntry']
export type StorageOperation = Schemas['StorageOperationResponse']
type ContentIndexCheck = Schemas['ContentIndexCheckResponse']
type DedupeResult = Schemas['DedupeResponse']

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
