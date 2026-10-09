import { describe, expect, it } from 'vitest'

import { filterImportFiles, IMPORT_ACCEPT, importPackageNameOf, isContainerFile } from './nzbImportRequest'

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
      file('g.txt'),
      file('h.rdlinks'),
      file('i.CRAWLJOB')
    ])
    expect(kept.map(entry => entry.name)).toEqual([
      'a.nzb',
      'b.torrent',
      'c.dlc',
      'D.DLC',
      'e.ccf',
      'f.rsdf',
      'g.txt',
      'h.rdlinks',
      'i.CRAWLJOB'
    ])
  })

  it('drops unrelated files', () => {
    expect(filterImportFiles([file('notes.md'), file('archive.rar')])).toEqual([])
  })
})

/** Every format the dialog takes is named once, for the picker, the drop zone and the dispatch (RD-1220-02). */
describe('the accepted formats', () => {
  it('offers every format in the file picker', () => {
    expect(IMPORT_ACCEPT).toBe('.nzb,.torrent,.dlc,.ccf,.rsdf,.txt,.rdlinks,.crawljob')
  })

  it('sends containers, link files and crawljobs to the container import, NZBs and torrents not', () => {
    for (const name of ['a.dlc', 'b.ccf', 'c.rsdf', 'd.txt', 'e.text', 'f.rdlinks', 'g.crawljob', 'H.CRAWLJOB']) {
      expect(isContainerFile(name), name).toBe(true)
    }
    for (const name of ['a.nzb', 'b.torrent', 'c.crawljob.bak']) {
      expect(isContainerFile(name), name).toBe(false)
    }
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
    expect(importPackageNameOf('Season.rdlinks')).toBe('Season')
    expect(importPackageNameOf('Season.crawljob')).toBe('Season')
  })

  it('strips the password marker and keeps the bare name', () => {
    expect(importPackageNameOf('Season{{secret}}.dlc')).toBe('Season')
  })

  it('falls back to the file name when nothing is left', () => {
    expect(importPackageNameOf('.dlc')).toBe('.dlc')
  })
})
