import { describe, expect, it } from 'vitest'

// The pure filtering + handoff-state logic lives in `nzbImportRequest.ts` (no side-effecting
// imports), separate from the `useNzbDropZone` composable itself (window listeners, router,
// toast) so it can be unit-tested without a Nuxt/Vue app context.
import { claimFileDrops, consumeFileImportRequest, fileDropClaim, filterImportFiles, requestFileImport } from './nzbImportRequest'

function file(name: string): File {
  return new File(['content'], name)
}

describe('filterImportFiles', () => {
  it('keeps NZB and torrent files case-insensitively and drops the rest', () => {
    const files = [file('one.nzb'), file('two.NZB'), file('three.torrent'), file('four.TORRENT'), file('five.rar')]
    expect(filterImportFiles(files).map(f => f.name)).toEqual(['one.nzb', 'two.NZB', 'three.torrent', 'four.TORRENT'])
  })

  it('returns an empty array when nothing matches', () => {
    expect(filterImportFiles([file('one.md'), file('two.rar')])).toEqual([])
  })

  it('returns an empty array for empty input', () => {
    expect(filterImportFiles([])).toEqual([])
  })
})

describe('nzb import request handoff', () => {
  it('consume-once: a request is returned once, then a second consume returns null', () => {
    const files = [file('a.nzb')]
    requestFileImport(files)
    expect(consumeFileImportRequest()).toEqual({ files })
    expect(consumeFileImportRequest()).toBeNull()
  })

  it('defaults to an empty file list, for the future keyboard-shortcut opener', () => {
    requestFileImport()
    expect(consumeFileImportRequest()).toEqual({ files: [] })
  })

  it('returns null when no request is pending', () => {
    expect(consumeFileImportRequest()).toBeNull()
  })
})

describe('a page that claims dropped files (RD-120-51)', () => {
  it('holds the claim until it releases it, and an older holder cannot release a newer claim', () => {
    const first = (): void => {}
    const second = (): void => {}
    const releaseFirst = claimFileDrops(first)
    expect(fileDropClaim()).toBe(first)
    const releaseSecond = claimFileDrops(second)
    releaseFirst()
    expect(fileDropClaim()).toBe(second)
    releaseSecond()
    expect(fileDropClaim()).toBeNull()
  })
})
