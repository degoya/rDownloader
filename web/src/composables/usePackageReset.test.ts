/**
 * "Reset the package completely" (RD-1390-07): what the confirmation names — the package, its
 * number of files and whether the unpacked results go — and that nothing is asked of the server
 * without it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock } from '@/test/mocks'
import { defineComponent, h } from 'vue'

import { api } from '@/api/client'
import type { Download, DownloadPackage } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { useTransfersStore } from '@/stores/transfers'
import { mountComponent } from '@/test/mount'

import type { ConfirmOptions } from './useConfirm'
import { usePackageReset } from './usePackageReset'

const settings = vi.hoisted(() => ({ value: { package_reset_delete_extracted: true } as Record<string, unknown> }))
vi.mock('@/api/client', () => apiClientMock({
  GET: vi.fn(async (path: string) => ({ data: path === '/api/v1/settings' ? settings.value : [] })),
  responseError: vi.fn(() => 'refused'),
  errorMessage: vi.fn(),
  resultMessage: vi.fn(() => 'Done')
}))
vi.mock('@/composables/useEventStream', () => eventStreamMock())

const confirm = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))

const FILES = [
  { id: 'a', file_name: 'set.part1.rar', state: 'failed', package_id: 'p1' },
  { id: 'b', file_name: 'set.part2.rar', state: 'completed', package_id: 'p1' },
  { id: 'c', file_name: 'other.bin', state: 'queued', package_id: 'p2' }
] as unknown as Download[]

function mountReset() {
  let composable: ReturnType<typeof usePackageReset> | null = null
  const Harness = defineComponent({
    setup() {
      composable = usePackageReset()
      return () => h('div')
    }
  })
  mountComponent(Harness, { messages: { downloads } })
  const transfers = useTransfersStore()
  transfers.downloads = FILES
  transfers.packages = [{ id: 'p1', name: 'Broken Set' }] as unknown as DownloadPackage[]
  return { reset: composable! as ReturnType<typeof usePackageReset>, transfers }
}

function resetCalls(): unknown[] {
  return (vi.mocked(api.POST).mock.calls as unknown as [string, unknown][])
    .filter(([path]) => path === '/api/v1/packages/{id}/reset')
    .map(([, init]) => init)
}

beforeEach(() => {
  vi.mocked(api.POST).mockReset()
  vi.mocked(api.POST).mockResolvedValue({ data: { code: 'package.reset', message: 'reset', params: { count: '2' } } } as never)
  confirm.mockClear()
  confirm.mockResolvedValue(true)
  settings.value = { package_reset_delete_extracted: true }
})

describe('usePackageReset', () => {
  it('names the package and its files, then resets it and says so', async () => {
    const { reset, transfers } = mountReset()
    await reset.resetPackage('p1')
    const [[options]] = confirm.mock.calls as unknown as [[ConfirmOptions]]
    expect(options.title).toBe(downloads.package_reset.title)
    expect(options.description).toContain('Broken Set')
    expect(options.description).toContain('2 files and everything unpacking made of them')
    expect(options.destructive).toBe(true)
    expect(resetCalls()).toEqual([{ params: { path: { id: 'p1' } } }])
    expect(transfers.notice).toBe('Package reset: 2 files download again from the start')
  })

  it('says the unpacked results stay while the setting keeps them', async () => {
    settings.value = { package_reset_delete_extracted: false }
    const { reset } = mountReset()
    await reset.resetPackage('p1')
    const [[options]] = confirm.mock.calls as unknown as [[ConfirmOptions]]
    expect(options.description).toContain('what unpacking made stays')
  })

  it('asks nothing of the server when the confirmation is declined', async () => {
    confirm.mockResolvedValue(false)
    const { reset } = mountReset()
    await reset.resetPackage('p1')
    expect(resetCalls()).toEqual([])
  })

  it('shows a refusal as the error', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'package.reset_running' } } as never)
    const { reset, transfers } = mountReset()
    await reset.resetPackage('p1')
    expect(transfers.error).toBe('refused')
  })
})
