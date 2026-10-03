import { describe, expect, it } from 'vitest'

import { readProviderReturn, startUrl, withoutProviderReturn } from './identityProvider'

describe('identity provider navigations', () => {
  it('asks the start to come back to the page the sign-in screen covered', () => {
    expect(startUrl('/')).toBe('/api/v1/auth/oidc/start')
    expect(startUrl('/downloads?view=packages')).toBe(
      '/api/v1/auth/oidc/start?return_to=%2Fdownloads%3Fview%3Dpackages'
    )
  })

  it('reads a refusal, the reported name and a finished link from the address', () => {
    expect(readProviderReturn('?oidc_error=auth.oidc_not_administrator&oidc_name=Jane+Doe')).toEqual({
      error: 'auth.oidc_not_administrator',
      name: 'Jane Doe',
      linked: false
    })
    expect(readProviderReturn('?oidc=linked').linked).toBe(true)
    expect(readProviderReturn('').error).toBeNull()
  })

  it('removes only what the redirect back added', () => {
    expect(
      withoutProviderReturn('https://dl.example.com/settings/security?tab=signin&oidc=linked#x')
    ).toBe('/settings/security?tab=signin#x')
    expect(withoutProviderReturn('https://dl.example.com/?oidc_error=a&oidc_name=b')).toBe('/')
  })
})
