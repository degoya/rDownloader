/**
 * Exporting packages as a link file (RD-1210-01): the dialog's choice goes to the service, the
 * file the service wrote is saved under the name it gave, and the toast counts what went in.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const add = vi.fn()
const open = vi.fn()
const saved: { blob: Blob, name: string }[] = []

vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add }),
  useOverlay: () => ({ create: () => ({ open }) })
}))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}` })
}))
vi.mock('@/components/PackageExportModal.vue', () => ({ default: {} }))
vi.mock('@/utils/jsonFile', () => ({ downloadBlob: (blob: Blob, name: string) => saved.push({ blob, name }) }))
vi.mock('@/api/client', () => ({
  api: { POST: vi.fn() },
  errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed',
  resultMessage: (body: { code?: string }) => `translated ${body.code ?? ''}`
}))

const { api } = await import('@/api/client')
const { exportFailures, exportFileName, usePackageExport } = await import('./usePackageExport')

function answer(headers: Record<string, string>, data: Blob | undefined, error?: unknown) {
  return { data, error, response: new Response(null, { headers }) }
}

describe('exporting packages', () => {
  beforeEach(() => {
    add.mockReset()
    open.mockReset()
    saved.length = 0
    vi.mocked(api.POST).mockReset()
  })

  it('sends the choice, saves the file under its name and counts the links', async () => {
    open.mockReturnValue({ result: Promise.resolve({ format: 'rdlinks', passphrase: 'correct horse' }) })
    const file = new Blob(['{}'])
    vi.mocked(api.POST).mockResolvedValue(answer({
      'content-disposition': 'attachment; filename="rdownloader-20261008-120000.rdlinks"',
      'x-rd-export-links': '3',
      'x-rd-export-skipped': '1'
    }, file) as never)

    await usePackageExport().exportPackages({ packageIds: ['p1'], downloadIds: ['d1'] })

    expect(api.POST).toHaveBeenCalledWith('/api/v1/packages/export', {
      body: {
        package_ids: ['p1'],
        download_ids: ['d1'],
        collector_package_ids: [],
        all: false,
        format: 'rdlinks',
        passphrase: 'correct horse'
      },
      parseAs: 'blob'
    })
    expect(saved).toEqual([{ blob: file, name: 'rdownloader-20261008-120000.rdlinks' }])
    const toast = add.mock.calls[0]?.[0] as { title: string, description: string, color: string }
    expect(toast.title).toContain('"count":3')
    expect(toast.description).toContain('"count":1')
    expect(toast.color).toBe('warning')
  })

  it('counts the embedded NZBs and names the ones it could not fetch, with the reason', async () => {
    open.mockReturnValue({ result: Promise.resolve({ format: 'rdlinks', passphrase: '' }) })
    const failed = [{ name: 'Show.S01E01', message: 'limit', code: 'collector.nzb_rejected', params: { reason: 'limit' } }]
    vi.mocked(api.POST).mockResolvedValue(answer({
      'x-rd-export-links': '0',
      'x-rd-export-nzbs': '2',
      'x-rd-export-skipped': '1',
      'x-rd-export-failed': encodeURIComponent(JSON.stringify(failed))
    }, new Blob(['{}'])) as never)

    await usePackageExport().exportPackages({ collectorPackageIds: ['c1'] })

    const toast = add.mock.calls[0]?.[0] as { description: string, color: string }
    expect(toast.description).toContain('common.export.nzbs {"count":2}')
    expect(toast.description).toContain('common.export.nzb_failed')
    expect(toast.description).toContain('Show.S01E01 (translated collector.nzb_rejected)')
    expect(toast.color).toBe('warning')
  })

  it('reads the failure header defensively', () => {
    expect(exportFailures(null)).toEqual([])
    expect(exportFailures('')).toEqual([])
    expect(exportFailures('%E0%A4%A')).toEqual([])
    expect(exportFailures(encodeURIComponent('{"name":"x"}'))).toEqual([])
    expect(exportFailures(encodeURIComponent('[{"name":"\u00dc","code":"export.nzb_unavailable","message":"m"},{"bogus":1}]')))
      .toEqual([{ name: '\u00dc', code: 'export.nzb_unavailable', message: 'm' }])
  })

  it('sends no passphrase it was not given and does nothing when the dialog is closed', async () => {
    open.mockReturnValue({ result: Promise.resolve(null) })
    await usePackageExport().exportPackages({ all: true })
    expect(api.POST).not.toHaveBeenCalled()

    open.mockReturnValue({ result: Promise.resolve({ format: 'crawljob', passphrase: '' }) })
    vi.mocked(api.POST).mockResolvedValue(answer({}, undefined, 'export.nothing_exportable') as never)
    await usePackageExport().exportPackages({ all: true })
    const body = (vi.mocked(api.POST).mock.calls[0]?.[1] as unknown as { body: Record<string, unknown> }).body
    expect(body).not.toHaveProperty('passphrase')
    expect(saved).toEqual([])
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'error', description: 'export.nothing_exportable' })
  })

  it('names a file the service named none for by the date and the format', () => {
    expect(exportFileName('attachment; filename="x.crawljob"', 'crawljob')).toBe('x.crawljob')
    expect(exportFileName(null, 'rdlinks')).toMatch(/^rdownloader-\d{4}-\d{2}-\d{2}\.rdlinks$/)
  })
})
