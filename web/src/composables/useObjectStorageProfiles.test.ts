import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ObjectStorageProfile } from '@/api/types'

const client = vi.hoisted(() => ({
  GET: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
  DELETE: vi.fn()
}))
vi.mock('@/api/client', () => ({
  api: client,
  responseError: (response: { error?: { code?: string } }) => `failed:${response.error?.code ?? ''}`,
  resultMessage: () => 'done'
}))

const {
  bucketLink,
  credentialSources,
  dropsSecretForHost,
  emptyForm,
  enabledObjectStorageProfiles,
  endpointLabel,
  formComplete,
  formFor,
  isIncomplete,
  keepsSecret,
  testOutcome,
  toCreateBody,
  toUpdateBody,
  uploadRemoteFor,
  useObjectStorageProfiles
} = await import('./useObjectStorageProfiles')

function profile(overrides: Partial<ObjectStorageProfile> = {}): ObjectStorageProfile {
  return {
    id: 'p1',
    name: 'Archive',
    provider: 's3',
    endpoint: 'https://minio.example:9000',
    region: 'us-east-1',
    bucket: 'archive',
    addressing: 'path',
    credential_source: 'static',
    access_key_id: 'AKIAEXAMPLE',
    account: null,
    has_secret: true,
    has_session_token: true,
    checksums: true,
    enabled: true,
    created_at: '2026-09-27T00:00:00Z',
    updated_at: '2026-09-27T00:00:00Z',
    ambient_custom_endpoint: false,
    ...overrides
  }
}

describe('object storage form', () => {
  it('never reads a stored secret or session token back into the form', () => {
    const form = formFor(profile())
    expect(form.secret_access_key).toBe('')
    expect(form.session_token).toBe('')
    expect(form.access_key_id).toBe('AKIAEXAMPLE')
    expect(form.addressing).toBe('path')
  })

  it('starts a new profile with stored keys, checksums on and the server-chosen addressing', () => {
    const form = emptyForm()
    expect(form.credential_source).toBe('static')
    expect(form.checksums).toBe(true)
    expect(form.enabled).toBe(true)
    // Automatic addressing is omitted, so the server picks it from the endpoint.
    expect('addressing' in toCreateBody({ ...form, name: 'x' })).toBe(false)
    expect(toCreateBody({ ...form, name: 'x', addressing: 'virtual_host' }).addressing).toBe('virtual_host')
  })

  it('sends blank optional fields as null and trims the rest', () => {
    const body = toCreateBody({ ...emptyForm(), name: ' Archive ', endpoint: '  ', region: ' eu-central-1 ', bucket: '' })
    expect(body.name).toBe('Archive')
    expect(body.endpoint).toBeNull()
    expect(body.region).toBe('eu-central-1')
    expect(body.bucket).toBeNull()
    expect(body.provider).toBe('s3')
  })

  it('sends keys only for the static source', () => {
    // Ambient and anonymous profiles store nothing; a key typed before switching must not travel.
    const typed = { ...emptyForm(), name: 'x', access_key_id: 'AKIA', secret_access_key: 'secret', session_token: 'token' }
    for (const source of ['ambient', 'anonymous'] as const) {
      const body = toCreateBody({ ...typed, credential_source: source })
      expect(body.access_key_id).toBeNull()
      expect(body.secret_access_key).toBeNull()
      expect(body.session_token).toBeNull()
    }
    const body = toCreateBody(typed)
    expect(body.access_key_id).toBe('AKIA')
    expect(body.secret_access_key).toBe('secret')
    expect(body.session_token).toBe('token')
  })

  it('keeps a stored secret on an update by sending nothing for it', () => {
    const body = toUpdateBody(formFor(profile()))
    expect(body.secret_access_key).toBeNull()
    expect(body.session_token).toBeNull()
    expect(body.clear_session_token).toBe(false)
  })

  it('clears the session token only when asked and no new one was typed', () => {
    const form = { ...formFor(profile()), clear_session_token: true }
    expect(toUpdateBody(form).clear_session_token).toBe(true)
    expect(toUpdateBody({ ...form, session_token: 'new' }).clear_session_token).toBe(false)
    expect(toUpdateBody({ ...form, credential_source: 'ambient' }).clear_session_token).toBe(false)
  })

  it('asks for both keys on a new static profile, and only for the key id when one is stored', () => {
    const named = { ...emptyForm(), name: 'Archive' }
    expect(formComplete({ ...emptyForm() }, null)).toBe(false)
    expect(formComplete(named, null)).toBe(false)
    expect(formComplete({ ...named, access_key_id: 'AKIA' }, null)).toBe(false)
    expect(formComplete({ ...named, access_key_id: 'AKIA', secret_access_key: 's' }, null)).toBe(true)
    expect(formComplete({ ...named, access_key_id: 'AKIA', endpoint: 'https://minio.example:9000/' }, profile())).toBe(true)
    // Switching an ambient profile to static needs a secret now.
    expect(formComplete({ ...named, access_key_id: 'AKIA' }, profile({ has_secret: false }))).toBe(false)
    expect(formComplete({ ...named, credential_source: 'ambient' }, null)).toBe(true)
    expect(formComplete({ ...named, credential_source: 'anonymous' }, null)).toBe(true)
  })

  it('offers a shared access signature to Azure alone', () => {
    expect(credentialSources('azure')).toEqual(['static', 'shared_access_signature', 'ambient', 'anonymous'])
    expect(credentialSources('s3')).not.toContain('shared_access_signature')
    expect(credentialSources('gcs')).not.toContain('shared_access_signature')
  })

  it('sends Azure its account and secret and nothing that belongs to S3', () => {
    const typed = {
      ...emptyForm(), name: 'Blob', provider: 'azure' as const, credential_source: 'shared_access_signature' as const,
      account: ' mediaarchive ', region: 'eu-central-1', addressing: 'virtual_host' as const,
      access_key_id: 'AKIA', secret_access_key: 'sv=1&sig=x', session_token: 'token'
    }
    const body = toCreateBody(typed)
    expect(body).toMatchObject({
      provider: 'azure', account: 'mediaarchive', secret_access_key: 'sv=1&sig=x',
      region: null, access_key_id: null, session_token: null, checksums: false
    })
    expect('addressing' in body).toBe(false)
    expect(toUpdateBody({ ...typed, clear_session_token: true }).clear_session_token).toBe(false)
    // Google takes its key file as the secret and no account.
    const google = toCreateBody({ ...typed, provider: 'gcs', credential_source: 'static', secret_access_key: '{"type":"service_account"}' })
    expect(google.account).toBeNull()
    expect(google.secret_access_key).toBe('{"type":"service_account"}')
    // Ambient and anonymous sources carry no secret at all.
    expect(toCreateBody({ ...typed, credential_source: 'ambient' }).secret_access_key).toBeNull()
  })

  it('asks Azure for its account and keeps a stored secret only for the same provider and source', () => {
    const azure = { ...emptyForm(), name: 'Blob', provider: 'azure' as const, credential_source: 'static' as const }
    expect(formComplete({ ...azure, secret_access_key: 'a2V5' }, null)).toBe(false)
    expect(formComplete({ ...azure, account: 'media', secret_access_key: 'a2V5' }, null)).toBe(true)
    const stored = profile({ provider: 'azure', endpoint: null, account: 'media', access_key_id: null })
    expect(formComplete({ ...azure, account: 'media' }, stored)).toBe(true)
    // An account key does not become a signature by switching the source.
    expect(formComplete({ ...azure, account: 'media', credential_source: 'shared_access_signature' }, stored)).toBe(false)
    // Nor does an S3 secret become a Google key.
    expect(formComplete({ ...emptyForm(), name: 'G', provider: 'gcs' }, profile())).toBe(false)
    expect(formComplete({ ...emptyForm(), name: 'G', provider: 'gcs', secret_access_key: '{}' }, profile())).toBe(true)
  })
})

describe('object storage secrets and hosts (RD-1190-20)', () => {
  it('keeps a stored secret only for the host it was typed for', () => {
    const form = formFor(profile())
    expect(keepsSecret(form, profile())).toBe(true)
    expect(dropsSecretForHost(form, profile())).toBe(false)
    const moved = { ...form, endpoint: 'https://collector.example' }
    expect(keepsSecret(moved, profile())).toBe(false)
    expect(dropsSecretForHost(moved, profile())).toBe(true)
    expect(formComplete(moved, profile())).toBe(false)
    expect(formComplete({ ...moved, secret_access_key: 'again' }, profile())).toBe(true)
    // Without an endpoint the Azure account names the host.
    const blob = profile({ provider: 'azure', endpoint: null, account: 'media', credential_source: 'shared_access_signature' })
    expect(keepsSecret(formFor(blob), blob)).toBe(true)
    expect(dropsSecretForHost({ ...formFor(blob), account: 'other' }, blob)).toBe(true)
    // Another source is a different secret, not a moved one.
    expect(dropsSecretForHost({ ...form, credential_source: 'ambient' }, profile())).toBe(false)
  })

  it('sends machine credentials to an endpoint only with the explicit yes', () => {
    const ambient = { ...emptyForm(), name: 'Machine', credential_source: 'ambient' as const, endpoint: 'https://minio.example' }
    expect(formComplete(ambient, null)).toBe(false)
    expect(formComplete({ ...ambient, ambient_custom_endpoint: true }, null)).toBe(true)
    expect(toCreateBody({ ...ambient, ambient_custom_endpoint: true }).ambient_custom_endpoint).toBe(true)
    // The yes travels only where it means something.
    expect(toCreateBody({ ...ambient, endpoint: '', ambient_custom_endpoint: true }).ambient_custom_endpoint).toBe(false)
    expect(toCreateBody({ ...ambient, credential_source: 'static', ambient_custom_endpoint: true }).ambient_custom_endpoint).toBe(false)
    expect(formFor(profile({ credential_source: 'ambient', ambient_custom_endpoint: true })).ambient_custom_endpoint).toBe(true)
  })
})

describe('object storage labels', () => {
  it('names the endpoint host, or AWS S3 and its region', () => {
    expect(endpointLabel(profile())).toBe('minio.example:9000')
    expect(endpointLabel(profile({ endpoint: null, region: 'eu-west-1' }))).toBe('AWS S3 · eu-west-1')
    expect(endpointLabel(profile({ endpoint: null, region: null }))).toBe('AWS S3')
    expect(endpointLabel(profile({ provider: 'azure', endpoint: null, account: 'media' }))).toBe('media.blob.core.windows.net')
    expect(endpointLabel(profile({ provider: 'gcs', endpoint: null }))).toBe('storage.googleapis.com')
  })

  it('writes a bound bucket as a link in its provider\'s scheme', () => {
    expect(bucketLink(profile())).toBe('s3://archive')
    expect(bucketLink(profile({ provider: 'azure', bucket: 'media' }))).toBe('az://media')
    expect(bucketLink(profile({ provider: 'gcs', bucket: 'media_bucket' }))).toBe('gs://media_bucket')
    expect(bucketLink(profile({ bucket: null }))).toBeNull()
  })

  it('flags a static profile without a stored secret', () => {
    expect(isIncomplete(profile())).toBe(false)
    expect(isIncomplete(profile({ has_secret: false }))).toBe(true)
    expect(isIncomplete(profile({ credential_source: 'ambient', has_secret: false, access_key_id: null }))).toBe(false)
    // Azure and Google have no key id; the secret alone decides.
    expect(isIncomplete(profile({ provider: 'gcs', access_key_id: null }))).toBe(false)
    expect(isIncomplete(profile({ provider: 'azure', credential_source: 'shared_access_signature', has_secret: false }))).toBe(true)
  })

  it('builds the upload target post-processing understands', () => {
    expect(uploadRemoteFor('p1')).toBe('object-storage:p1/')
    expect(uploadRemoteFor('p1', '/incoming/')).toBe('object-storage:p1/incoming/')
  })
})

describe('test outcome', () => {
  it('passes the server code and its parameters through', () => {
    expect(testOutcome({ reachable: true, authenticated: false, code: 'object_storage.access_denied', params: { bucket: 'b' } }))
      .toEqual({ ok: false, code: 'object_storage.access_denied', params: { bucket: 'b' } })
  })

  it('names the failed step when no code came with it', () => {
    expect(testOutcome({ reachable: false, authenticated: false, code: null, params: {} }))
      .toEqual({ ok: false, code: 'object_storage.connect_failed', params: {} })
    expect(testOutcome({ reachable: true, authenticated: false, code: null, params: {} }))
      .toEqual({ ok: false, code: 'object_storage.auth_failed', params: {} })
    expect(testOutcome({ reachable: true, authenticated: true, code: null, params: {} })).toEqual({ ok: true })
  })
})

describe('useObjectStorageProfiles', () => {
  beforeEach(() => {
    for (const call of Object.values(client)) call.mockReset()
  })

  it('lists, creates, updates and removes profiles against the REST paths', async () => {
    client.GET.mockResolvedValue({ data: [profile()] })
    client.POST.mockResolvedValue({ data: profile({ id: 'p2', name: 'Second' }) })
    client.PUT.mockResolvedValue({ data: profile({ name: 'Renamed' }) })
    client.DELETE.mockResolvedValue({ data: { message: 'gone' } })
    const store = useObjectStorageProfiles()

    await store.refresh()
    expect(client.GET).toHaveBeenCalledWith('/api/v1/object-storage/profiles')
    expect(store.loading.value).toBe(false)
    expect(store.profiles.value.map(entry => entry.id)).toEqual(['p1'])

    await store.create({ ...emptyForm(), name: 'Second', access_key_id: 'AKIA', secret_access_key: 's' })
    expect(client.POST.mock.calls[0]?.[0]).toBe('/api/v1/object-storage/profiles')
    expect(store.profiles.value.map(entry => entry.id)).toEqual(['p1', 'p2'])

    await store.update('p1', { ...formFor(profile()), name: 'Renamed' })
    expect(client.PUT).toHaveBeenCalledWith('/api/v1/object-storage/profiles/{id}', expect.objectContaining({
      params: { path: { id: 'p1' } }
    }))
    expect(store.profiles.value[0]?.name).toBe('Renamed')

    expect(await store.remove('p2')).toBe(true)
    expect(client.DELETE).toHaveBeenCalledWith('/api/v1/object-storage/profiles/{id}', { params: { path: { id: 'p2' } } })
    expect(store.profiles.value.map(entry => entry.id)).toEqual(['p1'])
    expect(store.message.value).toBe('done')
  })

  it('keeps the list and reports the code when a save is refused', async () => {
    client.POST.mockResolvedValue({ error: { code: 'object_storage.profile_duplicate' } })
    const store = useObjectStorageProfiles()

    expect(await store.create({ ...emptyForm(), name: 'Dup' })).toBeNull()
    expect(store.error.value).toBe('failed:object_storage.profile_duplicate')
    expect(store.profiles.value).toEqual([])
  })

  it('keeps the row when a delete is refused', async () => {
    client.GET.mockResolvedValue({ data: [profile()] })
    client.DELETE.mockResolvedValue({ error: { code: 'object_storage.profile_not_found' } })
    const store = useObjectStorageProfiles()
    await store.refresh()

    expect(await store.remove('p1')).toBe(false)
    expect(store.profiles.value).toHaveLength(1)
    expect(store.error.value).toBe('failed:object_storage.profile_not_found')
  })

  it('runs the test on the profile route and answers the outcome', async () => {
    client.POST.mockResolvedValue({ data: { reachable: true, authenticated: true, code: null, params: {} } })
    const store = useObjectStorageProfiles()

    expect(await store.test('p1')).toEqual({ ok: true })
    expect(client.POST).toHaveBeenCalledWith('/api/v1/object-storage/profiles/{id}/test', { params: { path: { id: 'p1' } } })
    expect(store.busyId.value).toBeNull()
  })

  it('offers only enabled profiles to a picker, and none when the read fails', async () => {
    client.GET.mockResolvedValue({ data: [profile(), profile({ id: 'p2', enabled: false })] })
    expect((await enabledObjectStorageProfiles()).map(entry => entry.id)).toEqual(['p1'])

    client.GET.mockResolvedValue({ error: { code: 'auth.forbidden' } })
    expect(await enabledObjectStorageProfiles()).toEqual([])
  })
})
