/**
 * The shapes the collector store and its modules share: a package change, an intake, the
 * outcome of an enqueue, and the request body a package change turns into (RD-140-27).
 */
import type { CollectorPackageUpdateRequest, DownloadPriority, GrabberEntryRef, PostprocessLevel } from '@/api/types'

export interface CollectorPackageChange {
  name?: string
  categoryId?: string | null
  priority?: DownloadPriority
  /** null clears the password */
  password?: string | null
  /** null clears the package level (inherit category/global default) */
  postprocessLevel?: PostprocessLevel | null
  /** null clears the package script (inherit category/global default) */
  script?: string | null
}

/**
 * One row of the LinkGrabber's manual order. The order spans both kinds of entry.
 *
 * The shape is the contract's, not a copy of it: this was hand-written while the endpoint was
 * still being built, and a second declaration of a generated type is one that drifts from it
 * silently.
 */
export type GrabberOrderEntry = GrabberEntryRef

export interface IntakeInput {
  text: string
  packageName?: string
  password?: string
}

export interface IntakeOutcome {
  ok: boolean
  /** Links dropped by the domain blocklist before candidates were created. */
  skippedExcluded: number
  /** Links refused because their transfer service is switched off. */
  skippedDisabled: number
  /** Addresses the folder crawlers and site rules returned for this paste. */
  crawledFound: number
  /**
   * How many of those were refused: nothing claims them and no probe confirmed a file
   * behind them (RD-110-07). Reported rather than swallowed — a rule whose pattern reaches
   * one element too far otherwise looks exactly like an empty page.
   */
  crawledDropped: number
}

/** Outcome of a batch enqueue, including partial failures and account-less files. */
export interface EnqueueBatchResult {
  created: number
  failed: number
  firstError: string | null
  /** Files enqueued without a provider account (free/direct download attempt). */
  freeDownloadFiles: number
}

export function changeBody(change: CollectorPackageChange): CollectorPackageUpdateRequest {
  return {
    ...(change.name ? { name: change.name } : {}),
    ...(change.categoryId ? { category_id: change.categoryId } : {}),
    ...(change.categoryId === null ? { clear_category: true } : {}),
    ...(change.priority ? { priority: change.priority } : {}),
    ...(change.password ? { password: change.password } : {}),
    ...(change.password === null ? { clear_password: true } : {}),
    ...(change.postprocessLevel ? { postprocess_level: change.postprocessLevel } : {}),
    ...(change.postprocessLevel === null ? { clear_postprocess_level: true } : {}),
    ...(change.script ? { script: change.script } : {}),
    ...(change.script === null ? { clear_script: true } : {})
  }
}
