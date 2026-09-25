import { describe, expect, it, vi } from 'vitest'

// The composable pulls in the modal (and with it Nuxt UI's runtime); the diff helper under
// test needs none of that.
vi.mock('@nuxt/ui/composables', () => ({ useOverlay: () => ({ create: () => ({}) }) }))
vi.mock('@/components/PackageEditModal.vue', () => ({ default: {} }))

import { packageEditChange, type PackageEditResult } from './usePackageEdit'

function result(overrides: Partial<PackageEditResult> = {}): PackageEditResult {
  return {
    name: 'Release',
    password: null,
    clearPassword: false,
    postprocessLevel: null,
    script: null,
    renameFolder: false,
    ...overrides
  }
}

describe('packageEditChange', () => {
  it('sends nothing when the prefilled password comes back unchanged', () => {
    const pkg = { name: 'Release', has_password: true, password: 'secret' }
    expect(packageEditChange(pkg, result({ password: 'secret' }))).toEqual({})
  })

  it('sends the new value when the password was edited', () => {
    const pkg = { name: 'Release', has_password: true, password: 'secret' }
    expect(packageEditChange(pkg, result({ password: 'other' }))).toEqual({ password: 'other' })
  })

  it('clears the password when the field was emptied', () => {
    const pkg = { name: 'Release', has_password: true, password: 'secret' }
    expect(packageEditChange(pkg, result())).toEqual({ password: null })
  })

  it('clears the password when the switch was used', () => {
    const pkg = { name: 'Release', has_password: true, password: 'secret' }
    expect(packageEditChange(pkg, result({ password: 'secret', clearPassword: true }))).toEqual({ password: null })
  })

  it('adds a password to a package that had none', () => {
    const pkg = { name: 'Release', has_password: false }
    expect(packageEditChange(pkg, result({ password: 'new' }))).toEqual({ password: 'new' })
  })
})
