import { describe, expect, it } from 'vitest'

import { expiresInDays, tokenExpired, tokenExpiryItems, tokenExpiryLabel } from './tokenExpiry'

const t = (key: string, values?: Record<string, unknown>) => (values ? `${key}:${JSON.stringify(values)}` : key)
const NOW = Date.parse('2026-10-05T12:00:00Z')

describe('token expiry', () => {
  it('offers never first and sends no expiry for it', () => {
    const items = tokenExpiryItems(t)
    expect(items[0]).toEqual({ label: 'system.token_expiry.never', value: 0 })
    expect(items.map((item) => item.value)).toEqual([0, 7, 30, 90, 365])
    expect(expiresInDays(0)).toBeNull()
    expect(expiresInDays(30)).toBe(30)
  })

  it('tells an expired token from a current one and from one without an expiry', () => {
    expect(tokenExpired(null, NOW)).toBe(false)
    expect(tokenExpired('2026-10-05T11:59:00Z', NOW)).toBe(true)
    expect(tokenExpired('2026-10-06T12:00:00Z', NOW)).toBe(false)
    expect(tokenExpiryLabel(undefined, t, NOW)).toBeNull()
    expect(tokenExpiryLabel('2026-10-01T00:00:00Z', t, NOW)).toBe('system.token_expiry.expired')
    expect(tokenExpiryLabel('2026-11-01T00:00:00Z', t, NOW)).toMatch(/^system\.token_expiry\.expires_on:/)
  })
})
