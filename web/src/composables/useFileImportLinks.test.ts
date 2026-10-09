/**
 * An exported link file goes in through the container import (RD-1210-01), with its passphrase
 * and the choice to queue it once checked as fields of the same upload.
 */
import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const add = vi.fn()

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key })
}))
vi.mock('@/composables/useNzbImportModal', () => ({
  useFileImportModal: () => async () => ({
    entries: [{ file: new File(['{}'], 'holiday.rdlinks'), name: '' }],
    categoryId: null,
    priority: 'normal',
    passphrase: 'correct horse',
    enqueue: true
  })
}))
vi.mock('@/stores/collector', () => ({ useCollectorStore: () => ({ refresh: vi.fn(async () => {}) }) }))
vi.mock('@/stores/nzbImports', () => ({ useNzbImportsStore: () => ({ importMany: vi.fn(), importNzb: vi.fn() }) }))
vi.mock('@/api/client', () => ({
  api: { POST: vi.fn(async () => ({ data: { candidates: [{}] } })) },
  errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed'
}))

const { api } = await import('@/api/client')
const { useFileImport } = await import('./useFileImport')

describe('importing an exported link file', () => {
  it('uploads it as a container with its passphrase and the enqueue choice', async () => {
    await useFileImport(ref([])).importFiles()

    const [path, init] = vi.mocked(api.POST).mock.calls[0] as unknown as [string, { body: FormData }]
    expect(path).toBe('/api/v1/containers/import')
    expect(init.body.get('passphrase')).toBe('correct horse')
    expect(init.body.get('enqueue')).toBe('true')
    expect((init.body.get('file') as File).name).toBe('holiday.rdlinks')
  })
})
