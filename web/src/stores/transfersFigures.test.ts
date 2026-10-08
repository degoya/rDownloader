import { describe, expect, it } from 'vitest'
import { ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'

import { useTransferFigures } from './transfersFigures'

function pkg(state: string): DownloadPackage {
  return { id: 'pkg-1', name: 'Dr.House S03', state } as unknown as DownloadPackage
}

function file(id: string, state: string): Download {
  return { id, package_id: 'pkg-1', state, committed_bytes: '0', total_bytes: null } as unknown as Download
}

function complete(state: string, files: Download[]): boolean {
  const figures = useTransferFigures(ref(files), ref([pkg(state)]), ref({}))
  return figures.packageComplete.value['pkg-1'] ?? false
}

/**
 * RD-1190-13: the owner's queue showed "5/14 · 9 errors" with the finished tick, because the
 * package state alone counted. The files decide now.
 */
describe('packageComplete', () => {
  it('is never finished while a file waits, failed or is blocked, whatever the package state says', () => {
    for (const missing of ['retry_wait', 'failed', 'blocked', 'queued', 'paused', 'downloading']) {
      expect(complete('completed', [file('a', 'completed'), file('b', missing)]), missing).toBe(false)
    }
  })

  it('is finished once every file completed or stood down as a mirror', () => {
    expect(complete('queued', [file('a', 'completed'), file('b', 'completed')])).toBe(true)
    expect(complete('completed', [file('a', 'completed'), file('b', 'skipped')])).toBe(true)
  })

  it('needs at least one completed file, and trusts the state only without files', () => {
    expect(complete('completed', [file('a', 'skipped')])).toBe(false)
    expect(complete('completed', [])).toBe(true)
    expect(complete('queued', [])).toBe(false)
  })
})
