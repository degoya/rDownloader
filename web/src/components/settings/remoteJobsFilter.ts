/**
 * The remote jobs list's filter, and what clearing it reaches (RD-1200-01).
 *
 * The same rules as `RemoteJobService::clear` on the server, so the number in the question is the
 * number the request acts on: a job's provider is its account's, a job whose account is gone has
 * none and matches no provider filter, and a job still running at the provider is left out.
 */
import type { Account, RemoteJob, RemoteJobState } from '@/api/types'

/** `'all'` or one provider slug, `'all'` or one state: what the two selects above the list hold. */
export interface RemoteJobFilter {
  provider: string
  state: RemoteJobState | 'all'
}

/** Still running at the provider: never cleared, in either variant. */
export const RUNNING_STATES: readonly RemoteJobState[] = ['submitting', 'preparing', 'working']

export function providerOf(job: RemoteJob, accounts: readonly Account[]): string | null {
  return accounts.find(account => account.id === job.account_id)?.provider.toLowerCase() ?? null
}

export function filterJobs(jobs: readonly RemoteJob[], accounts: readonly Account[], filter: RemoteJobFilter): RemoteJob[] {
  return jobs.filter(job =>
    (filter.provider === 'all' || providerOf(job, accounts) === filter.provider)
    && (filter.state === 'all' || job.state === filter.state))
}

/** The providers the list's jobs run at, sorted, for the provider select. */
export function jobProviders(jobs: readonly RemoteJob[], accounts: readonly Account[]): string[] {
  const slugs = new Set<string>()
  for (const job of jobs) {
    const slug = providerOf(job, accounts)
    if (slug) slugs.add(slug)
  }
  return [...slugs].sort()
}

export interface ClearSelection {
  /** The jobs a clear removes, or tries to. */
  targets: RemoteJob[]
  /** The jobs of the filtered list left out because they are still running. */
  running: RemoteJob[]
  /** The providers of the targets, sorted; a job without an account adds none. */
  providers: string[]
}

export function clearSelection(visible: readonly RemoteJob[], accounts: readonly Account[]): ClearSelection {
  const running = visible.filter(job => RUNNING_STATES.includes(job.state))
  const targets = visible.filter(job => !RUNNING_STATES.includes(job.state))
  return { targets, running, providers: jobProviders(targets, accounts) }
}

/** The request body for the filter, as `POST /api/v1/remote-jobs/clear` takes it. */
export function clearBody(filter: RemoteJobFilter, atProvider: boolean) {
  return {
    confirmed: true,
    at_provider: atProvider,
    ...(filter.provider === 'all' ? {} : { provider: filter.provider }),
    states: filter.state === 'all' ? [] : [filter.state]
  }
}
