/**
 * The vocabulary the transfers store and its modules share: which states count as what, the
 * shapes of a package change and a clear, and the translation shorthand (RD-140-27).
 */
import type { components } from '@/api/schema'
import type { DownloadPriority, PackageUpdateRequest, PostprocessLevel } from '@/api/types'
import { i18n } from '@/i18n'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'

export interface DownloadSelection {
  categoryId?: string | undefined
  accountId?: string | undefined
  proxyProfileId?: string | undefined
  priority?: DownloadPriority | undefined
}

/** `everything` also removes working packages, stopping them first (RD-180-21). */
export type ClearScope = components['schemas']['PackageClearScope']

/** A package the server refused to clear, with the stable code saying why. */

export type ClearResult = components['schemas']['PackageClearResponse']

export interface PackageChange {
  categoryId?: string | null
  priority?: DownloadPriority
  name?: string
  /** null clears the stored password */
  password?: string | null
  /** null clears the package level (inherit category/global default) */
  postprocessLevel?: PostprocessLevel | null
  /** null clears the package script (inherit category/global default) */
  script?: string | null
}

export const ACTIVE_STATES = ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'] as const

/** States a pause acts on. Overlaps `RESUMABLE_STATES`, so pause always wins in the UI toggle. */
export const PAUSABLE_STATES: readonly string[] = ['queued', 'retry_wait', ...ACTIVE_STATES]
/** States a resume acts on. `queued` is left out – a queued transfer is already on its way. */
// `skipped` belongs here: a waiting mirror is started by resuming it, which stands its
// siblings down. Without it there is no way to choose a different link by hand.
export const RESUMABLE_STATES: readonly string[] = ['retry_wait', 'paused', 'failed', 'blocked', 'cancelled', 'skipped']
/**
 * States a reset acts on: everything that is not moving right now. A finished or seeding job is
 * included on purpose — starting over is exactly what a reset is for.
 */
export const RESETTABLE_STATES: readonly string[] = ['queued', 'retry_wait', 'paused', 'failed', 'blocked', 'cancelled', 'completed', 'seeding']
/** A transfer that stopped and needs attention. */
export const FAILED_STATES: readonly string[] = ['failed', 'blocked']
/** States whose files still count towards the outstanding queue volume. */
export const PENDING_STATES: readonly string[] = [...PAUSABLE_STATES, 'paused']
/**
 * States whose bytes are still going to come down the wire — the same line the server draws
 * for its estimate (`is_transferring` in `download_handlers.rs`). Paused, blocked, verifying,
 * repairing, extracting and seeding are all outstanding in some sense, but none of them is
 * being fetched, so none of them belongs in "how long at the current speed".
 */
export const TRANSFERRING_STATES: readonly string[] = ['queued', 'retry_wait', 'resolving', 'downloading']

export const t = (key: string, named: Record<string, unknown> = {}, plural?: number): string =>
  plural === undefined ? i18n.global.t(key, named) : i18n.global.t(key, named, plural)

export function changeBody(change: PackageChange): PackageUpdateRequest {
  return {
    ...(change.categoryId ? { category_id: change.categoryId } : {}),
    ...(change.categoryId === null ? { clear_category: true } : {}),
    ...(change.priority ? { priority: change.priority } : {}),
    ...(change.name ? { name: change.name } : {}),
    ...(change.password ? { password: change.password } : {}),
    ...(change.password === null ? { clear_password: true } : {}),
    ...(change.postprocessLevel ? { postprocess_level: change.postprocessLevel } : {}),
    ...(change.postprocessLevel === null ? { clear_postprocess_level: true } : {}),
    ...(change.script ? { script: change.script } : {}),
    ...(change.script === null ? { clear_script: true } : {})
  }
}

/**
 * What a bulk action refused, translated by code and each reason once, or `null` when every file
 * was done. The coded `refusals` are read loosely so an answer without them still shows the
 * English `errors`.
 */
export function bulkRefusals(result: { errors: string[], refusals?: unknown }): string | null {
  const coded = result.refusals
  const messages = Array.isArray(coded) && coded.length
    ? coded.map(refusal => translateServerMessage(serverMessageFrom(refusal)))
    : result.errors
  const distinct = [...new Set(messages)]
  return distinct.length ? distinct.slice(0, 3).join(' · ') : null
}

/** The refusal in a failed removal's body, translated by its code when the catalogue knows it. */
export function payloadError(value: unknown): string {
  const message = serverMessageFrom(value)
  return message ? translateServerMessage(message) : t('downloads.notices.remove_failed')
}
