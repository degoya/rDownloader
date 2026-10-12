import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Download, DownloadPackage } from '@/api/types'
import { setLocale } from '@/i18n'

import { QUEUE_DONE_SPACING_MS, createQueueAnnouncer } from './transfersEvents'

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }))

vi.mock('@/composables/useNotifications', () => ({
  useNotifications: () => ({ notify })
}))

function file(id: string, state: string, packageId = 'pkg-1'): Download {
  return { id, package_id: packageId, file_name: `${id}.bin`, state } as unknown as Download
}

function fifty(state: (index: number) => string, packageId = 'pkg-1'): Download[] {
  return Array.from({ length: 50 }, (_, index) => file(`${packageId}-f${index}`, state(index), packageId))
}

const packages = [{ id: 'pkg-1', name: 'Holiday' }, { id: 'pkg-2', name: 'Second' }] as unknown as DownloadPackage[]

/**
 * The counted cases of RD-1260-04, at the tab: what one tab hands its notifier (which then leaves
 * a message to push or to the leading tab, `desktopNotifier.test.ts`).
 */
describe('queue announcer: notifications counted', () => {
  beforeEach(() => {
    void setLocale('en')
    notify.mockReset()
  })

  it('reports fifty failed files of one package once, when the package is settled', () => {
    const announce = createQueueAnnouncer(() => 0)
    announce(fifty(() => 'downloading'), packages)
    // They fail over several refreshes while the rest still runs: nothing yet.
    announce(fifty(index => (index < 20 ? 'failed' : 'downloading')), packages)
    announce(fifty(index => (index < 40 ? 'failed' : 'downloading')), packages)
    expect(notify).not.toHaveBeenCalled()
    announce(fifty(() => 'failed'), packages)
    announce(fifty(() => 'failed'), packages)

    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith(
      'Downloads failed: Holiday',
      '50 of 50 files could not be downloaded.',
      { tag: 'package-failed:pkg-1', pushEvent: 'package_failed' }
    )
  })

  it('leaves a package with finished files to the tab alone, counting what failed', () => {
    const announce = createQueueAnnouncer(() => 0)
    announce(fifty(() => 'downloading'), packages)
    announce(fifty(index => (index < 3 ? 'failed' : 'completed')), packages)

    expect(notify).toHaveBeenCalledWith(
      'Downloads failed: Holiday',
      '3 of 50 files could not be downloaded.',
      { tag: 'package-failed:pkg-1' }
    )
    // And the queue is empty with work done: the one "finished" beside it.
    expect(notify).toHaveBeenCalledTimes(2)
  })

  it('says nothing when the queue is paused rather than finished', () => {
    const announce = createQueueAnnouncer(() => 0)
    announce([file('a', 'downloading'), file('b', 'downloading')], packages)
    announce([file('a', 'completed'), file('b', 'paused')], packages)
    expect(notify).not.toHaveBeenCalled()
    announce([file('a', 'completed'), file('b', 'completed')], packages)
    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith('Download list finished', '2 files finished.', { tag: 'queue-done' })
  })

  it('announces a finished queue at most once in ten minutes', () => {
    let now = 0
    const announce = createQueueAnnouncer(() => now)
    for (const at of [0, 60_000, 5 * 60_000, QUEUE_DONE_SPACING_MS + 1]) {
      now = at
      announce([file(`run-${at}`, 'downloading')], packages)
      announce([file(`run-${at}`, 'completed')], packages)
    }
    expect(notify).toHaveBeenCalledTimes(2)
  })

  it('says nothing over ten retry waits and reports the failure that ends them once', () => {
    const announce = createQueueAnnouncer(() => 0)
    for (let attempt = 0; attempt < 10; attempt += 1) {
      announce([file('a', 'downloading')], packages)
      announce([file('a', 'retry_wait')], packages)
    }
    expect(notify).not.toHaveBeenCalled()
    announce([file('a', 'failed')], packages)
    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith('Download failed', '“a.bin” could not be downloaded.', {
      tag: 'package-failed:pkg-1',
      pushEvent: 'package_failed'
    })
  })

  it('reports two packages that fail as two notifications', () => {
    const announce = createQueueAnnouncer(() => 0)
    const both = (state: string) => [...fifty(() => state, 'pkg-1'), ...fifty(() => state, 'pkg-2')]
    announce(both('downloading'), packages)
    announce(both('failed'), packages)
    expect(notify).toHaveBeenCalledTimes(2)
  })
})
