import { describe, expect, it } from 'vitest'

import { filterImportFiles, importPackageNameOf } from './nzbImportRequest'

function file(name: string): File {
  return new File(['x'], name)
}

describe('filterImportFiles', () => {
  it('keeps every supported metadata file, whatever the case', () => {
    const kept = filterImportFiles([
      file('a.nzb'),
      file('b.torrent'),
      file('c.dlc'),
      file('D.DLC'),
      file('e.ccf'),
      file('f.rsdf'),
      file('g.txt')
    ])
    expect(kept.map(entry => entry.name)).toEqual([
      'a.nzb',
      'b.torrent',
      'c.dlc',
      'D.DLC',
      'e.ccf',
      'f.rsdf',
      'g.txt'
    ])
  })

  it('drops unrelated files', () => {
    expect(filterImportFiles([file('notes.md'), file('archive.rar')])).toEqual([])
  })
})

describe('importPackageNameOf', () => {
  it('strips the suffix of every supported format', () => {
    expect(importPackageNameOf('Season.nzb')).toBe('Season')
    expect(importPackageNameOf('Season.torrent')).toBe('Season')
    expect(importPackageNameOf('Season.dlc')).toBe('Season')
    expect(importPackageNameOf('Season.DLC')).toBe('Season')
    expect(importPackageNameOf('Season.rsdf')).toBe('Season')
    expect(importPackageNameOf('Season.txt')).toBe('Season')
  })

  it('strips the password marker and keeps the bare name', () => {
    expect(importPackageNameOf('Season{{secret}}.dlc')).toBe('Season')
  })

  it('falls back to the file name when nothing is left', () => {
    expect(importPackageNameOf('.dlc')).toBe('.dlc')
  })
})
