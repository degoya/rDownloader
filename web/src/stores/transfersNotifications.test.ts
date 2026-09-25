import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { Download } from '@/api/types'
import { setLocale } from '@/i18n'

import { useTransfersStore } from './transfers'

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }))

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The queue could not be read'),
  resultMessage: vi.fn(() => '')
}))

vi.mock('@/composables/useNotifications', () => ({
  useNotifications: () => ({
    supported: true,
    enabled: { value: true },
    permission: { value: 'granted' },
    enable: async () => true,
    disable: () => {},
    notify
  })
}))

/** A queue row as the API sends it; `recovery` is left out for a server that predates it. */
function file(name: string, state: string, recovery?: boolean): Download {
  return {
    id: name,
    package_id: 'pkg-1',
    file_name: name,
    state,
    committed_bytes: '0',
    total_bytes: '100',
    ...(recovery === undefined ? {} : { recovery })
  } as unknown as Download
}

/** Answers the three requests one `refresh()` makes, with the given queue snapshot. */
function snapshot(downloads: Download[]): void {
  vi.mocked(api.GET).mockImplementation((async (path: string) => {
    if (path === '/api/v1/downloads') return { data: downloads }
    if (path === '/api/v1/packages') return { data: [{ id: 'pkg-1', state: 'downloading' }] }
    return { data: { downloads: [], bytes_per_second: 0 } }
  }) as never)
}

/**
 * The false alarm from RD-107-10: a `vol…par2` volume that expired on the servers raised
 * "Download failed" for a package that went on to unpack cleanly. The notification is the
 * part the user actually saw, so it is pinned here — together with the boundary, a payload
 * file that is genuinely gone and still has to be reported.
 */
describe('transfers store: failure notifications', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    setLocale('en')
    notify.mockReset()
    vi.mocked(api.GET).mockReset()
  })

  it('says nothing about a recovery volume that was never needed', async () => {
    const store = useTransfersStore()
    snapshot([file('Release.vol012+10.par2', 'downloading', true)])
    await store.refresh()
    snapshot([file('Release.vol012+10.par2', 'failed', true)])
    await store.refresh()

    expect(notify).not.toHaveBeenCalled()
  })

  it('still reports a payload file that is gone', async () => {
    const store = useTransfersStore()
    snapshot([
      file('Release.part03.rar', 'downloading', false),
      file('Release.vol012+10.par2', 'downloading', true)
    ])
    await store.refresh()
    snapshot([
      file('Release.part03.rar', 'failed', false),
      file('Release.vol012+10.par2', 'failed', true)
    ])
    await store.refresh()

    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith(
      'Download failed',
      '“Release.part03.rar” could not be downloaded.'
    )
  })

  it('falls back to the file name when the server sends no marking', async () => {
    const store = useTransfersStore()
    snapshot([file('Release.vol012+10.par2', 'downloading')])
    await store.refresh()
    snapshot([file('Release.vol012+10.par2', 'failed')])
    await store.refresh()

    expect(notify).not.toHaveBeenCalled()
  })
})
