/**
 * Signing in through an identity provider (RD-190-15): the browser navigations the flow is made of.
 *
 * The start and the callback are not API calls but page loads — the service answers them with
 * redirects to the provider and back — so this module only builds their addresses and leaves the
 * page. A refused sign-in comes back as `?oidc_error=<code>` (and `?oidc_name=` for an account
 * that is not the administrator); a link as `?oidc=linked`.
 */
import { withBase } from '@/basePath'

/** Leaves the application for `url`. A function of its own so tests can stand in for it. */
export function leaveFor(url: string): void {
  window.location.assign(url)
}

/** The start of a sign-in that comes back to `returnTo`, a path inside the application. */
export function startUrl(returnTo: string): string {
  const query = returnTo && returnTo !== '/' ? `?return_to=${encodeURIComponent(returnTo)}` : ''
  return withBase(`/api/v1/auth/oidc/start${query}`)
}

/** What the provider's redirect back left in the address, if anything. */
export interface ProviderReturn {
  /** The stable code of a refused sign-in or link. */
  error: string | null
  /** The name the provider reported for an account that is not the administrator. */
  name: string | null
  /** A link finished. */
  linked: boolean
}

export function readProviderReturn(search: string): ProviderReturn {
  const params = new URLSearchParams(search)
  return {
    error: params.get('oidc_error'),
    name: params.get('oidc_name'),
    linked: params.get('oidc') === 'linked'
  }
}

/**
 * The address without what the provider's redirect back added, so a reload does not show the
 * same refusal again.
 */
export function withoutProviderReturn(href: string): string {
  const url = new URL(href)
  for (const name of ['oidc_error', 'oidc_name', 'oidc']) url.searchParams.delete(name)
  return `${url.pathname}${url.search}${url.hash}`
}
