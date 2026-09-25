import { describe, expect, it } from 'vitest'

import type { AuthProfile } from '@/api/types'

import { emptyForm, formFor, isExpired, isUsable, matchFor, scopeLabel, toCreateBody, toUpdateBody } from './useAuthProfiles'

function profile(overrides: Partial<AuthProfile> = {}): AuthProfile {
  return {
    id: '018f0000-0000-7000-8000-000000000001',
    name: 'Intranet',
    host: 'example.com',
    include_subdomains: false,
    path_prefix: null,
    method: 'bearer',
    origin: 'manual',
    enabled: true,
    expires_at: null,
    username: null,
    has_secret: true,
    has_client_certificate: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides
  } as AuthProfile
}

describe('form mapping', () => {
  it('never carries a stored credential back into the form', () => {
    // Secrets are write-only server-side, so an edit form must start blank rather than
    // pretending to show what is stored.
    const form = formFor(profile({ username: 'someone', method: 'basic' }))
    expect(form.secret).toBe('')
    expect(form.certificate_pem).toBe('')
    expect(form.username).toBe('someone')
  })

  it('rebuilds the scope string from host and path prefix', () => {
    expect(formFor(profile({ host: 'files.example.com', path_prefix: '/reports' })).scope)
      .toBe('files.example.com/reports')
    expect(formFor(profile()).scope).toBe('example.com')
  })

  it('sends a username only for basic authentication', () => {
    const form = { ...emptyForm(), name: 'x', scope: 'example.com', username: 'someone', secret: 'token' }
    expect(toCreateBody({ ...form, method: 'bearer' }).username).toBeNull()
    expect(toCreateBody({ ...form, method: 'basic' }).username).toBe('someone')
  })

  it('turns empty credential fields into null so the server keeps what it has', () => {
    const body = toCreateBody({ ...emptyForm(), name: ' Intranet ', scope: ' example.com ', secret: '  ' })
    expect(body.secret).toBeNull()
    expect(body.certificate_pem).toBeNull()
    expect(body.name).toBe('Intranet')
    expect(body.scope).toBe('example.com')
  })

  it('expands a date to the end of that day', () => {
    // "expires on the 5th" means the profile still works during the 5th.
    expect(toCreateBody({ ...emptyForm(), expires_at: '2026-03-05' }).expires_at)
      .toBe('2026-03-05T23:59:59.000Z')
    expect(toCreateBody(emptyForm()).expires_at).toBeNull()
  })

  it('passes the certificate clear flag through only on update', () => {
    expect(toUpdateBody(emptyForm(), true).clear_certificate).toBe(true)
    expect(toUpdateBody(emptyForm(), false).clear_certificate).toBe(false)
  })
})

describe('expiry', () => {
  const now = new Date('2026-03-05T12:00:00Z')

  it('treats a past expiry as expired and unusable', () => {
    const stale = profile({ expires_at: '2026-03-01T00:00:00Z' })
    expect(isExpired(stale, now)).toBe(true)
    expect(isUsable(stale, now)).toBe(false)
  })

  it('leaves a future or absent expiry alone', () => {
    expect(isExpired(profile({ expires_at: '2026-04-01T00:00:00Z' }), now)).toBe(false)
    expect(isExpired(profile(), now)).toBe(false)
  })

  it('counts a disabled profile as unusable even when it has not expired', () => {
    expect(isUsable(profile({ enabled: false }), now)).toBe(false)
  })
})

describe('scope preview', () => {
  it('marks a subdomain scope with a wildcard', () => {
    expect(scopeLabel(profile({ include_subdomains: true }))).toBe('*.example.com')
    expect(scopeLabel(profile({ path_prefix: '/reports' }))).toBe('example.com/reports')
  })
})

describe('matchFor mirrors the server rules', () => {
  it('picks the most specific scope', () => {
    const wide = profile({ id: 'a', host: 'example.com', include_subdomains: true })
    const narrow = profile({ id: 'b', host: 'cdn.example.com' })
    expect(matchFor([wide, narrow], 'https://cdn.example.com/f')?.id).toBe('b')
    expect(matchFor([wide, narrow], 'https://example.com/f')?.id).toBe('a')
  })

  it('prefers the longer path prefix on the same host', () => {
    const shallow = profile({ id: 'a', path_prefix: '/media' })
    const deep = profile({ id: 'b', path_prefix: '/media/hd' })
    expect(matchFor([shallow, deep], 'https://example.com/media/hd/clip')?.id).toBe('b')
    expect(matchFor([shallow, deep], 'https://example.com/media/sd/clip')?.id).toBe('a')
  })

  it('matches path prefixes on segment boundaries only', () => {
    const scoped = [profile({ path_prefix: '/media' })]
    expect(matchFor(scoped, 'https://example.com/media/clip')).not.toBeNull()
    // A plain prefix compare would wrongly match this.
    expect(matchFor(scoped, 'https://example.com/mediafoo')).toBeNull()
  })

  it('never matches a lookalike host', () => {
    const scoped = [profile({ include_subdomains: true })]
    for (const url of ['https://evil-example.com/f', 'https://example.com.evil.tld/f']) {
      expect(matchFor(scoped, url), url).toBeNull()
    }
  })

  it('skips profiles the server would not apply either', () => {
    expect(matchFor([profile({ enabled: false })], 'https://example.com/f')).toBeNull()
    expect(matchFor([profile({ expires_at: '2020-01-01T00:00:00Z' })], 'https://example.com/f')).toBeNull()
  })

  it('returns null for input that is not a URL', () => {
    expect(matchFor([profile()], 'not a url')).toBeNull()
  })
})
