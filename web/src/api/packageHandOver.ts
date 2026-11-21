/**
 * A LinkGrabber package handed to a remote-job provider (RD-1440-07):
 * `POST /api/v1/collector/packages/{id}/remote-job`.
 *
 * Through the shared coded `call`, so a refusal keeps its code. The types are written by hand
 * until the schema is regenerated from the service; the path is cast for the same reason.
 */
import type { RemoteJob, Settings } from './types'
import { call, type CallPath } from './call'

/** One link that reached the provider, with its job. */
export interface CandidateHandedOver {
  candidate_id: string
  job: RemoteJob
  /** The account already had this job, or the link was marked with it: nothing was sent. */
  already_running: boolean
}

/** One link that stayed in the LinkGrabber, and the coded reason. */
export interface CandidateStayed {
  candidate_id: string
  code: string
  message: string
}

export interface CollectorPackageHandOver {
  handed_over: CandidateHandedOver[]
  stayed: CandidateStayed[]
  /** The links handed over left the LinkGrabber (the setting), or stay marked with their job. */
  links_removed: boolean
  /** The package left with its last link. */
  package_removed: boolean
}

/** Where a link went that stayed after its hand-over; the regenerated `LinkCandidate` carries it. */
export interface CandidateHandOverMark {
  remote_job_id: string
  account_id: string
}

/** The settings document with the two switches the regenerated schema will carry. */
export type PackageHandOverSettings = Settings & {
  linkgrabber_hand_over_enabled?: boolean
  linkgrabber_hand_over_remove_links?: boolean
}

export const handOverCollectorPackage = (packageId: string, accountId: string) =>
  call<CollectorPackageHandOver>(
    'POST',
    `/api/v1/collector/packages/${encodeURIComponent(packageId)}/remote-job` as CallPath,
    { account_id: accountId }
  )
