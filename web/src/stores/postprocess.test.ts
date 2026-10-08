/**
 * The post-processing queue and its choices as the pipeline page reads them: one request for
 * concurrent refreshes, the script names and step plugins asked once, progress events applied.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { usePostprocessStore } from './postprocess'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

function entry(packageId: string) {
  return { package_id: packageId, stage: 'repairing', percent: null, current: null, pending: true } as never
}

describe('postprocess store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
  })

  it('shares one request between concurrent refreshes and applies a progress event', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [entry('p1')] } as never)
    const store = usePostprocessStore()

    await Promise.all([store.refresh(), store.refresh()])

    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(1)
    expect(store.active).toBe(true)
    store.applyProgress('p1', 'extracting', 40, 'part01.rar')
    store.applyProgress('unknown', 'extracting', 90, null)
    expect(store.queue[0]).toMatchObject({ stage: 'extracting', percent: 40, current: 'part01.rar', pending: false })

    store.applyProgress('p1', null, null, null)
    expect(store.queue[0]?.stage).toBe('extracting')
  })

  it('keeps the queue it had and names the refusal when a refresh fails', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({ data: [entry('p1')] } as never)
    const store = usePostprocessStore()
    await store.refresh()
    vi.mocked(api.GET).mockResolvedValueOnce({ error: { code: 'service.unreachable' } } as never)

    await store.refresh()

    expect(store.error).toBe('The service did not answer')
    expect(store.queue).toHaveLength(1)
  })

  it('asks for the scripts once unless forced, and again after a failure', async () => {
    vi.mocked(api.GET)
      .mockResolvedValueOnce({ error: { code: 'postprocess.scripts_unreadable' } } as never)
      .mockResolvedValue({ data: { scripts: ['done.sh'], directory: '/scripts' } } as never)
    const store = usePostprocessStore()

    expect(await store.loadScripts()).toEqual([])
    expect(store.error).toBe('The service did not answer')
    expect(await store.loadScripts()).toEqual(['done.sh'])
    expect(await store.loadScripts()).toEqual(['done.sh'])
    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(2)
    expect(store.scriptsDirectory).toBe('/scripts')

    await store.loadScripts(true)
    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(3)
  })

  it('asks for the step plugins once', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [{ id: 'checksum' }] } as never)
    const store = usePostprocessStore()

    await store.loadPluginSteps()
    await store.loadPluginSteps()

    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(1)
    expect(store.pluginSteps).toEqual([{ id: 'checksum' }])
  })
})
