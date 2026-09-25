import { describe, expect, it } from 'vitest'

import type { RemoteCredential } from '@/api/types'
import {
  DEFAULT_PORTS,
  authModesFor,
  emptyForm,
  endpointLabel,
  formFor,
  hostKeyId,
  toCreateBody,
  toUpdateBody
} from './useRemoteCredentials'

function credential(overrides: Partial<RemoteCredential> = {}): RemoteCredential {
  return {
    id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b',
    name: 'Archive',
    protocol: 'ftp',
    host: 'files.example.com',
    port: 21,
    username: 'bob',
    auth_mode: 'password',
    passive: true,
    has_secret: true,
    has_key: false,
    has_passphrase: false,
    enabled: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides
  } as RemoteCredential
}

describe('remote credential form', () => {
  it('never reads a stored credential back into the form', () => {
    // The API cannot return them, so a form prefilled from a stored login has to start
    // blank; anything else would imply the value is known.
    const form = formFor(credential({ has_secret: true }))
    expect(form.secret).toBe('')
    expect(form.private_key).toBe('')
    expect(form.passphrase).toBe('')
    expect(form.username).toBe('bob')
  })

  it('shows a default port as empty and a custom one as a number', () => {
    expect(formFor(credential({ port: 21 })).port).toBe('')
    expect(formFor(credential({ port: 2121 })).port).toBe('2121')
    expect(formFor(credential({ protocol: 'sftp', port: 22 })).port).toBe('')
  })

  it('sends only the credential the chosen mode uses', () => {
    // Sending a password along with a key would store a credential the mode never uses,
    // and would keep it alive in the vault for no reason.
    const form = {
      ...emptyForm(),
      protocol: 'sftp' as const,
      auth_mode: 'private_key' as const,
      username: 'bob',
      secret: 'unused-password',
      private_key: '-----BEGIN OPENSSH PRIVATE KEY-----',
      passphrase: 'phrase'
    }
    const body = toCreateBody(form)
    expect(body.secret).toBeNull()
    expect(body.private_key).toBe('-----BEGIN OPENSSH PRIVATE KEY-----')
    expect(body.passphrase).toBe('phrase')
  })

  it('drops the user name for anonymous logins', () => {
    const body = toCreateBody({ ...emptyForm(), auth_mode: 'anonymous', username: 'ignored' })
    expect(body.username).toBeNull()
  })

  it('treats a blank or invalid port as the protocol default', () => {
    expect(toCreateBody({ ...emptyForm(), port: '' }).port).toBeNull()
    expect(toCreateBody({ ...emptyForm(), port: '   ' }).port).toBeNull()
    expect(toCreateBody({ ...emptyForm(), port: 'abc' }).port).toBeNull()
    expect(toCreateBody({ ...emptyForm(), port: '0' }).port).toBeNull()
    expect(toCreateBody({ ...emptyForm(), port: '99999' }).port).toBeNull()
    expect(toCreateBody({ ...emptyForm(), port: '2121' }).port).toBe(2121)
  })

  it('carries the clear-key flag only on update', () => {
    expect(toUpdateBody(emptyForm(), true).clear_private_key).toBe(true)
    expect(toUpdateBody(emptyForm(), false).clear_private_key).toBe(false)
  })
})

describe('protocol capabilities', () => {
  it('offers keys and agents for sftp only', () => {
    expect(authModesFor('sftp')).toEqual(['password', 'private_key', 'agent'])
    for (const protocol of ['ftp', 'ftps', 'ftps_implicit'] as const) {
      expect(authModesFor(protocol)).toEqual(['anonymous', 'password'])
    }
  })

  it('knows the default port of every protocol', () => {
    expect(DEFAULT_PORTS.ftp).toBe(21)
    expect(DEFAULT_PORTS.ftps).toBe(21)
    expect(DEFAULT_PORTS.ftps_implicit).toBe(990)
    expect(DEFAULT_PORTS.sftp).toBe(22)
  })
})

describe('labels', () => {
  it('shows the endpoint the way a link is matched against it', () => {
    expect(endpointLabel(credential())).toBe('bob@files.example.com')
    expect(endpointLabel(credential({ port: 2121 }))).toBe('bob@files.example.com:2121')
    expect(endpointLabel(credential({ username: null }))).toBe('files.example.com')
  })

  it('identifies a host key by endpoint and algorithm', () => {
    const key = {
      host: 'box.example',
      port: 22,
      algorithm: 'ssh-ed25519',
      fingerprint: 'SHA256:abc',
      first_seen: '2026-01-01T00:00:00Z'
    }
    expect(hostKeyId(key)).toBe('box.example:22:ssh-ed25519')
    // A second key type for the same server is a separate entry, not a replacement.
    expect(hostKeyId({ ...key, algorithm: 'rsa-sha2-512' })).not.toBe(hostKeyId(key))
  })
})
