/**
 * An exported link file goes in through the container import (RD-1210-01), with its passphrase
 * and the choice to queue it once checked as fields of the same upload. The NZBs it carries come
 * back as NZB imports, and a `.crawljob` takes the same road (RD-1220-02).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const add = vi.fn()
const nzbRefresh = vi.fn(async () => {})
let picked = 'holiday.rdlinks'

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string, values?: Record<string, unknown>) => `${key}${values && 'count' in values ? `:${String(values.count)}` : ''}` })
}))
vi.mock('@/composables/useNzbImportModal', () => ({
  useFileImportModal: () => async () => ({
    entries: [{ file: new File(['{}'], picked), name: '' }],
    categoryId: null,
    priority: 'normal',
    passphrase: 'correct horse',
    enqueue: true
  })
}))
vi.mock('@/stores/collector', () => ({ useCollectorStore: () => ({ refresh: vi.fn(async () => {}) }) }))
vi.mock('@/stores/nzbImports', () => ({ useNzbImportsStore: () => ({ importMany: vi.fn(), importNzb: vi.fn(), refresh: nzbRefresh }) }))
vi.mock('@/api/client', () => ({
  api: { POST: vi.fn(async () => ({ data: { candidates: [{}] } })) },
  errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed'
}))

const { api } = await import('@/api/client')
const { useFileImport } = await import('./useFileImport')

describe('importing an exported link file', () => {
  beforeEach(() => {
    picked = 'holiday.rdlinks'
    add.mockReset()
    nzbRefresh.mockReset()
    vi.mocked(api.POST).mockClear()
  })

  it('uploads it as a container with its passphrase and the enqueue choice', async () => {
    await useFileImport(ref([])).importFiles()

    const [path, init] = vi.mocked(api.POST).mock.calls[0] as unknown as [string, { body: FormData }]
    expect(path).toBe('/api/v1/containers/import')
    expect(init.body.get('passphrase')).toBe('correct horse')
    expect(init.body.get('enqueue')).toBe('true')
    expect((init.body.get('file') as File).name).toBe('holiday.rdlinks')
  })

  it('counts the NZBs the file carried and shows them in the LinkGrabber', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({ data: { candidates: [], nzb_imports: [{}, {}] } } as never)
    await useFileImport(ref([])).importFiles()

    expect(nzbRefresh).toHaveBeenCalledOnce()
    const toast = add.mock.calls[0]?.[0] as { description: string }
    expect(toast.description).toContain('linkgrabber.files.container_imported_description:0')
    expect(toast.description).toContain('linkgrabber.files.container_nzbs:2')
  })

  it('takes a crawljob through the container import too', async () => {
    picked = 'jdownloader.crawljob'
    await useFileImport(ref([])).importFiles()

    const [path, init] = vi.mocked(api.POST).mock.calls[0] as unknown as [string, { body: FormData }]
    expect(path).toBe('/api/v1/containers/import')
    expect((init.body.get('file') as File).name).toBe('jdownloader.crawljob')
    expect(nzbRefresh).not.toHaveBeenCalled()
  })
})
