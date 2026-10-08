/**
 * "Reset failed" over the download list and one package (RD-1190-15): what is asked first, and
 * what the bulk endpoint receives — a state filter where it names exactly the files on screen,
 * the ids where a name search shows only some of them.
 */
import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import { api } from '@/api/client'
import type { Download, DownloadState } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { useTransfersStore } from '@/stores/transfers'
import { mountComponent } from '@/test/mount'

import { useResetFailed } from './useResetFailed'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), POST: vi.fn() },
  responseError: vi.fn(() => 'refused'),
  errorMessage: vi.fn(),
  resultMessage: vi.fn(() => 'Done')
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

const confirm = vi.fn(async (_files: string[], _hasCompleted: boolean) => ({ confirmed: true, deleteFiles: false }))
vi.mock('@/composables/useResetConfirm', () => ({ useResetConfirm: () => confirm }))

function file(id: string, state: DownloadState, packageId = 'p1'): Download {
  return { id, file_name: `${id}.bin`, state, package_id: packageId } as unknown as Download
}

const LIST = [file('f1', 'failed'), file('b1', 'blocked'), file('q1', 'queued'), file('f2', 'failed', 'p2'), file('c1', 'cancelled')]

function mountReset(visible: Download[] = LIST, needle = '') {
  let composable: ReturnType<typeof useResetFailed> | null = null
  const Harness = defineComponent({
    setup() {
      composable = useResetFailed({ visible: ref(visible), needle: ref(needle) })
      return () => h('div')
    }
  })
  mountComponent(Harness, { messages: { downloads } })
  const transfers = useTransfersStore()
  transfers.downloads = LIST
  return { reset: composable! as ReturnType<typeof useResetFailed>, transfers }
}

function bulkBodies(): unknown[] {
  return (vi.mocked(api.POST).mock.calls as unknown as [string, { body: unknown }][])
    .filter(([path]) => path === '/api/v1/downloads/bulk')
    .map(([, init]) => init.body)
}

beforeEach(() => {
  vi.mocked(api.POST).mockReset()
  vi.mocked(api.POST).mockResolvedValue({ data: { affected: 2, errors: [], refusals: [] } } as never)
  confirm.mockClear()
})

describe('useResetFailed', () => {
  it('counts the failed and the blocked files of the shown list, cancelled ones aside', () => {
    const { reset } = mountReset()
    expect(reset.counts.value).toEqual({ failed: 2, blocked: 1, both: 3 })
  })

  it('asks by state when no search narrows the list, after naming the files', async () => {
    const { reset, transfers } = mountReset()
    await reset.resetShown('both')
    expect(confirm).toHaveBeenCalledWith(['f1.bin', 'b1.bin', 'f2.bin'], false)
    expect(bulkBodies()).toEqual([{ ids: [], action: 'reset', filter: { states: ['failed', 'blocked'] } }])
    expect(transfers.notice).toBe('2 files reset')
  })

  it('names only the states the list holds', async () => {
    const { reset } = mountReset()
    await reset.resetShown('failed')
    expect(bulkBodies()).toEqual([{ ids: [], action: 'reset', filter: { states: ['failed'] } }])
  })

  it('sends the ids when a name search shows only part of the list', async () => {
    const { reset } = mountReset([file('f1', 'failed'), file('f2', 'failed', 'p2')], 'f')
    await reset.resetShown('failed')
    expect(bulkBodies()).toEqual([{ ids: ['f1', 'f2'], action: 'reset' }])
  })

  it('takes every failed and blocked file of one package', async () => {
    const { reset } = mountReset([])
    await reset.resetPackage('p1')
    expect(confirm).toHaveBeenCalledWith(['f1.bin', 'b1.bin'], false)
    expect(bulkBodies()).toEqual([{ ids: [], action: 'reset', filter: { states: ['failed', 'blocked'], package_id: 'p1' } }])
  })

  it('sends nothing when the question is declined or nothing is stuck', async () => {
    confirm.mockResolvedValueOnce({ confirmed: false, deleteFiles: false })
    const { reset } = mountReset()
    await reset.resetShown('both')
    await flushPromises()
    expect(bulkBodies()).toEqual([])
    const empty = mountReset([file('q1', 'queued')])
    await empty.reset.resetShown('failed')
    expect(bulkBodies()).toEqual([])
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(empty.transfers.notice).toBe(downloads.notices.nothing_to_reset)
  })
})
