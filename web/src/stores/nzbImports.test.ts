import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { NzbImport } from '@/api/types'
import type { FileImportEntry } from '@/composables/useNzbImportModal'

import { useNzbImportsStore } from './nzbImports'

function nzbFile(name: string, sizeBytes = 1024): File {
  const file = new File(['x'.repeat(Math.min(sizeBytes, 1024))], name, { type: 'application/x-nzb' })
  Object.defineProperty(file, 'size', { value: sizeBytes })
  return file
}

function jsonResponse(ok: boolean, body: unknown): Response {
  return { ok, json: async () => body } as Response
}

describe('nzbImports store: importMany', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('aggregates created, duplicate and error outcomes without aborting the batch', async () => {
    const calls: string[] = []
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      calls.push(name)
      if (name === 'created.nzb') return jsonResponse(true, { id: '1', sha256: 'a', duplicate: false, name: 'created' })
      if (name === 'duplicate.nzb') return jsonResponse(true, { id: '2', sha256: 'b', duplicate: true, name: 'duplicate' })
      return jsonResponse(false, { error: 'server rejected the file' })
    })
    vi.stubGlobal('fetch', fetchMock)

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
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      order.push(`start:${name}`)
      await Promise.resolve()
      order.push(`end:${name}`)
      return jsonResponse(true, { id: name, sha256: name, duplicate: false, name })
    })
    vi.stubGlobal('fetch', fetchMock)

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
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const form = init?.body as FormData
      const name = (form.get('file') as File).name
      pendingDuringCalls.push(store.pending)
      return jsonResponse(true, { id: name, sha256: name, duplicate: false, name })
    })
    vi.stubGlobal('fetch', fetchMock)

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
    vi.stubGlobal('fetch', fetchMock)
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
