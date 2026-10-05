/**
 * The expiry of an API or capture token (RD-1110-07): chosen in days when it is minted, never
 * by default, and shown in the token lists. The server refuses an expired token like a revoked
 * one and keeps listing it, so the list is where somebody learns why a client stopped working.
 */
import { formatDay } from '@/utils/format'

/** The durations the forms offer, in days. `0` is "never", the default. */
export const TOKEN_EXPIRY_DAYS = [0, 7, 30, 90, 365] as const

type Translate = (key: string, values?: Record<string, unknown>) => string

/** The choices for a `USelect`: never first, then each duration. */
export function tokenExpiryItems(t: Translate): { label: string, value: number }[] {
  return TOKEN_EXPIRY_DAYS.map((days) => ({
    label: days === 0 ? t('system.token_expiry.never') : t('system.token_expiry.days', { count: days }),
    value: days
  }))
}

/** The request field: `null` for "never", which the server reads as no expiry. */
export function expiresInDays(days: number): number | null {
  return days > 0 ? days : null
}

/** Whether a token's expiry has passed. A token without one never expires. */
export function tokenExpired(expiresAt: string | null | undefined, now: number = Date.now()): boolean {
  return Boolean(expiresAt) && Date.parse(expiresAt as string) <= now
}

/** The line a token list shows for the expiry, or `null` for a token that never expires. */
export function tokenExpiryLabel(
  expiresAt: string | null | undefined,
  t: Translate,
  now: number = Date.now()
): string | null {
  if (!expiresAt) return null
  if (tokenExpired(expiresAt, now)) return t('system.token_expiry.expired')
  return t('system.token_expiry.expires_on', { date: formatDay(expiresAt) })
}
