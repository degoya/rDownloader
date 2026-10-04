import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { NzbImport } from '@/api/types'
import type { FileImportEntry } from '@/composables/useNzbImportModal'

import { useNzbImportsStore } from './nzbImports'

/** The handlers the store registered, so a test can play the server's events. */
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => { for (const name of Object.keys(registered)) delete handlers[name] }
  }
}))

function nzbFile(name: string, sizeBytes = 1024): File {
  const file = new File(['x'.repeat(Math.min(sizeBytes, 1024))], name, { type: 'application/x-nzb' })
  Object.defineProperty(file, 'size', { value: sizeBytes })
  return file
}

/** What the client hands back: the body on success, the parsed refusal otherwise. */
function jsonResponse(ok: boolean, body: unknown): { data?: unknown, error?: unknown } {
  return ok ? { data: body } : { error: body }
}

describe('nzbImports store: importMany', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('aggregates created, duplicate and error outcomes without aborting the batch', async () => {
    const calls: string[] = []
    const fetchMock = vi.fn(async (_path: string, init?: { body?: unknown }) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      calls.push(name)
      if (name === 'created.nzb') return jsonResponse(true, { id: '1', sha256: 'a', duplicate: false, name: 'created' })
      if (name === 'duplicate.nzb') return jsonResponse(true, { id: '2', sha256: 'b', duplicate: true, name: 'duplicate' })
      return jsonResponse(false, { error: 'server rejected the file' })
    })
    vi.spyOn(api, 'POST').mockImplementation(fetchMock as never)

    const store = useNzbImportsStore()
    const entries: FileImportEntry[] = [
      { file: nzbFile('created.nzb'), name: 'created' },
      { file: nzbFile('oversized.nzb', 70 * 1024 * 1024), name: 'oversized' },
      { file: nzbFile('duplicate.nzb'), name: 'duplicate' },
      { file: nzbFile('broken.nzb'), name: 'broken' }
    ]

    const result = await store.importMany(entries, { categoryId: null, priority: 'normal' })

    expect(result.created).toEqual([{ id: '1', sha256: 'a', duplicate: false, name: 'created' }])
    expect(result.duplicates).toEqual([{ id: '2', sha256: 'b', duplicate: true, name: 'duplicate' }])
    expect(result.errors).toEqual([
      { file: 'oversized.nzb', message: expect.stringContaining('64') },
      { file: 'broken.nzb', message: 'server rejected the file' }
    ])
    // The oversized file never reaches the network; it fails the client-side guard.
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })

  it('calls the endpoint once per file, sequentially and in order', async () => {
    const order: string[] = []
    const fetchMock = vi.fn(async (_path: string, init?: { body?: unknown }) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      order.push(`start:${name}`)
      await Promise.resolve()
      order.push(`end:${name}`)
      return jsonResponse(true, { id: name, sha256: name, duplicate: false, name })
    })
    vi.spyOn(api, 'POST').mockImplementation(fetchMock as never)

    const store = useNzbImportsStore()
    const entries: FileImportEntry[] = [
      { file: nzbFile('one.nzb'), name: 'one' },
      { file: nzbFile('two.nzb'), name: 'two' },
      { file: nzbFile('three.nzb'), name: 'three' }
    ]

    await store.importMany(entries, { categoryId: null, priority: 'normal' })

    // A concurrent (Promise.all) fan-out would interleave start/end; sequential fan-out never does.
    expect(order).toEqual([
      'start:one.nzb', 'end:one.nzb',
      'start:two.nzb', 'end:two.nzb',
      'start:three.nzb', 'end:three.nzb'
    ])
  })

  it('holds pending true for the whole batch instead of toggling per file', async () => {
    const pendingDuringCalls: boolean[] = []
    const store = useNzbImportsStore()
    const fetchMock = vi.fn(async (_path: string, init?: { body?: unknown }) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      pendingDuringCalls.push(store.pending)
      return jsonResponse(true, { id: name, sha256: name, duplicate: false, name })
    })
    vi.spyOn(api, 'POST').mockImplementation(fetchMock as never)

    const entries: FileImportEntry[] = [
      { file: nzbFile('one.nzb'), name: 'one' },
      { file: nzbFile('two.nzb'), name: 'two' }
    ]

    expect(store.pending).toBe(false)
    const promise = store.importMany(entries, { categoryId: null, priority: 'normal' })
    expect(store.pending).toBe(true)
    await promise

    expect(pendingDuringCalls).toEqual([true, true])
    expect(store.pending).toBe(false)
  })

  it('returns an empty result without touching pending for an empty batch', async () => {
    const fetchMock = vi.fn()
    vi.spyOn(api, 'POST').mockImplementation(fetchMock as never)
    const store = useNzbImportsStore()

    const result = await store.importMany([], { categoryId: null, priority: 'normal' })

    expect(result).toEqual({ created: [], duplicates: [], errors: [] })
    expect(fetchMock).not.toHaveBeenCalled()
    expect(store.pending).toBe(false)
  })
})

describe('nzbImports store: update', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('persists a changed category and replaces the local import', async () => {
    const original: NzbImport = {
      id: '01900000-0000-7000-8000-000000000001',
      name: 'release.nzb',
      sha256: 'a'.repeat(64),
      position: 1,
      state: 'imported',
      file_count: 1,
      segment_count: 1,
      total_bytes: '128',
      category_id: '01900000-0000-7000-8000-000000000002',
      priority: 'normal',
      import_mode: 'review',
      source_path: null,
      error: null,
      duplicate: false,
      has_password: false,
      created_at: '2026-09-02T10:00:00Z'
    }
    const changed = { ...original, category_id: '01900000-0000-7000-8000-000000000003' }
    vi.spyOn(api, 'PATCH').mockResolvedValue({ data: changed } as never)
    const store = useNzbImportsStore()
    store.imports = [original]

    expect(await store.update(original.id, { categoryId: changed.category_id })).toBe(true)

    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/nzb/imports/{id}', {
      params: { path: { id: original.id } },
      body: { category_id: changed.category_id }
    })
    expect(store.imports[0]?.category_id).toBe(changed.category_id)
  })

  it('uses the explicit clear flag for the default category', async () => {
    const changed = {
      id: '01900000-0000-7000-8000-000000000004',
      category_id: null
    }
    vi.spyOn(api, 'PATCH').mockResolvedValue({ data: changed } as never)
    const store = useNzbImportsStore()

    expect(await store.update(changed.id, { categoryId: null })).toBe(true)
    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/nzb/imports/{id}', {
      params: { path: { id: changed.id } },
      body: { clear_category: true }
    })
  })
})

/** RD-191-13: an import handed to a provider stays listed, marked; a refusal changes nothing. */
describe('nzbImports store: handOver', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('posts the account, replaces the row with the marked one and reports the duplicate guard', async () => {
    const store = useNzbImportsStore()
    const waiting = { id: 'nzb-1', sha256: 'a', name: 'Show.S01E01', state: 'imported', handed_over: null } as unknown as NzbImport
    store.imports = [waiting]
    const marked = { ...waiting, handed_over: { remote_job_id: 'job-1', account_id: 'acc-1' } }
    const post = vi.spyOn(api, 'POST').mockResolvedValue({ data: { import: marked, job: { id: 'job-1' }, already_running: true } } as never)

    const result = await store.handOver('nzb-1', 'acc-1')

    expect(post).toHaveBeenCalledWith('/api/v1/nzb/imports/{id}/remote-job', {
      params: { path: { id: 'nzb-1' } },
      body: { account_id: 'acc-1' }
    })
    expect(result).toEqual({ ok: true, item: marked, alreadyRunning: true })
    expect(store.imports).toEqual([marked])
    expect(store.handingOverIds.size).toBe(0)
  })

  it('hands a queued package over by its own route and marks the import behind it', async () => {
    const store = useNzbImportsStore()
    const queued = { id: 'nzb-1', sha256: 'a', name: 'Show.S01E01', state: 'enqueued', handed_over: null } as unknown as NzbImport
    store.imports = [queued]
    const marked = { ...queued, handed_over: { remote_job_id: 'job-1', account_id: 'acc-1' } }
    const post = vi.spyOn(api, 'POST').mockResolvedValue({ data: { import: marked, job: { id: 'job-1' }, already_running: false } } as never)

    const result = await store.handOverPackage('pkg-1', 'acc-1')

    expect(post).toHaveBeenCalledWith('/api/v1/packages/{id}/remote-job', {
      params: { path: { id: 'pkg-1' } },
      body: { account_id: 'acc-1' }
    })
    expect(result).toEqual({ ok: true, item: marked, alreadyRunning: false })
    expect(store.imports).toEqual([marked])
  })

  it('answers the refusal and leaves the row as it was', async () => {
    const store = useNzbImportsStore()
    const waiting = { id: 'nzb-1', sha256: 'a', name: 'Show.S01E01', state: 'imported', handed_over: null } as unknown as NzbImport
    store.imports = [waiting]
    vi.spyOn(api, 'POST').mockResolvedValue({ error: { code: 'nzb.remote_job_no_nzb', message: 'This account\'s provider does not take NZB files' }, response: { status: 400 } } as never)

    const result = await store.handOver('nzb-1', 'acc-1')

    expect(result.ok).toBe(false)
    expect(store.imports).toEqual([waiting])
    expect(store.handingOverIds.size).toBe(0)
  })
})

/**
 * RD-191-13: forgetting or discarding the remote job clears the badge on the server without a
 * collector event, so the store follows `remote_job.changed` — but only while a badge is shown.
 */
describe('nzbImports store: the hand-over badge follows its remote job', () => {
  const marked = { id: 'nzb-1', sha256: 'a', name: 'Show.S01E01', state: 'imported', handed_over: { remote_job_id: 'job-1', account_id: 'acc-1' } } as unknown as NzbImport
  const cleared = { ...marked, handed_over: null } as unknown as NzbImport

  function event(name: string): void {
    handlers[name]?.(new MessageEvent(name, { data: '{"entity":"remote_job","id":"job-1","removed":true}' }))
  }

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
  })

  afterEach(() => {
    useNzbImportsStore().disconnectEvents()
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('re-reads the imports once per burst and drops the badge of a forgotten job', async () => {
    const get = vi.spyOn(api, 'GET').mockResolvedValue({ data: [cleared] } as never)
    const store = useNzbImportsStore()
    store.imports = [marked]
    store.connectEvents()

    event('remote_job.changed')
    event('remote_job.changed')
    await vi.advanceTimersByTimeAsync(300)

    expect(get).toHaveBeenCalledOnce()
    expect(get).toHaveBeenCalledWith('/api/v1/nzb/imports')
    expect(store.imports[0]?.handed_over).toBeNull()
  })

  it('asks nothing while no import carries a badge', async () => {
    const get = vi.spyOn(api, 'GET').mockResolvedValue({ data: [cleared] } as never)
    const store = useNzbImportsStore()
    store.imports = [cleared]
    store.connectEvents()

    event('remote_job.changed')
    await vi.advanceTimersByTimeAsync(300)

    expect(get).not.toHaveBeenCalled()
  })

  it('stops following once disconnected', async () => {
    const get = vi.spyOn(api, 'GET').mockResolvedValue({ data: [cleared] } as never)
    const store = useNzbImportsStore()
    store.imports = [marked]
    store.connectEvents()
    store.disconnectEvents()

    event('remote_job.changed')
    await vi.advanceTimersByTimeAsync(300)

    expect(get).not.toHaveBeenCalled()
  })
})
