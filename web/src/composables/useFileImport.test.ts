/**
 * A batch of torrents and containers reports one outcome per file (WEB-02). The uploads ran in a
 * `Promise.all`, so one that threw discarded the results of every other file — uploaded ones too —
 * and the batch ended without a report.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const add = vi.fn()
const refresh = vi.fn(async () => {})
const entries = [
  { file: new File(['d'], 'one.torrent'), name: '' },
  { file: new File(['d'], 'two.torrent'), name: '' },
  { file: new File(['d'], 'links.dlc'), name: '' }
]

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}` })
}))
vi.mock('@/composables/useNzbImportModal', () => ({
  useFileImportModal: () => async () => ({ entries, categoryId: null, priority: 'normal' })
}))
vi.mock('@/stores/collector', () => ({ useCollectorStore: () => ({ refresh }) }))
vi.mock('@/stores/nzbImports', () => ({ useNzbImportsStore: () => ({ importMany: vi.fn(), importNzb: vi.fn() }) }))
vi.mock('@/api/client', () => ({
  api: { POST: vi.fn() },
  errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed'
}))

const { api } = await import('@/api/client')
const { useFileImport } = await import('./useFileImport')

describe('importing several torrents and containers', () => {
  beforeEach(() => {
    add.mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('keeps the results of the other files when one upload throws', async () => {
    vi.mocked(api.POST).mockImplementation((async (path: string, init: { body: FormData }) => {
      const name = (init.body.get('file') as File).name
      if (name === 'two.torrent') throw new Error('aborted')
      if (path === '/api/v1/containers/import') return { data: { candidates: [{}, {}] } }
      return { data: { candidates: [] } }
    }) as never)
    const { importFiles, importing } = useFileImport(ref([]))

    await importFiles()

    expect(api.POST).toHaveBeenCalledTimes(3)
    expect(add).toHaveBeenCalledTimes(1)
    const toast = add.mock.calls[0]?.[0] as { description: string, color: string }
    expect(toast.description).toContain('"created":2')
    expect(toast.description).toContain('"errors":1')
    expect(toast.description).toContain('aborted')
    expect(toast.color).toBe('error')
    expect(importing.value).toBe(false)
  })

  it('sends each file as multipart through the client', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { candidates: [] } } as never)
    const { importFiles } = useFileImport(ref([]))

    await importFiles()

    const paths = (vi.mocked(api.POST).mock.calls as unknown as [string, unknown][]).map(([path, init]) => [path, (init as { body: unknown }).body instanceof FormData])
    expect(paths).toEqual([
      ['/api/v1/torrents/import', true],
      ['/api/v1/torrents/import', true],
      ['/api/v1/containers/import', true]
    ])
  })
})
