import { afterEach, describe, expect, it } from 'vitest'

import { setLocale } from '@/i18n'
import { setByteDisplay, setByteUnit } from '@/utils/byteDisplay'

import { transferTitle } from './useDocumentTitle'

/**
 * The four cases RD-106-07 names, on the pure derivation rather than on a mounted application:
 * idle, one transfer, several, and switched off.
 */
describe('browser tab title', () => {
  afterEach(() => {
    setLocale('en')
    setByteUnit('auto')
    setByteDisplay('binary')
  })

  it('leaves the application name alone while nothing is running', () => {
    expect(transferTitle({ enabled: true, activeCount: 0, rate: 0 })).toBe('rDownloader')
  })

  it('reports the rate and the count for a single transfer', () => {
    expect(transferTitle({ enabled: true, activeCount: 1, rate: 1_572_864 }))
      .toBe('1.5 MiB/s · 1 active · rDownloader')
  })

  it('counts every running transfer', () => {
    expect(transferTitle({ enabled: true, activeCount: 4, rate: 5_242_880 }))
      .toBe('5.0 MiB/s · 4 active · rDownloader')
  })

  it('says nothing but the name once the setting is off', () => {
    expect(transferTitle({ enabled: false, activeCount: 4, rate: 5_242_880 })).toBe('rDownloader')
  })

  /**
   * The same restraint `formatDuration` shows: a rate that is not a usable figure is left out
   * rather than printed as a placeholder, so the tab never reads "— B/s".
   */
  it('drops a rate there is nothing honest to say about', () => {
    for (const rate of [0, null, undefined, Number.NaN, Number.POSITIVE_INFINITY, -1]) {
      expect(transferTitle({ enabled: true, activeCount: 2, rate })).toBe('2 active · rDownloader')
    }
  })

  it('follows the language and the byte preferences', () => {
    setLocale('de')
    expect(transferTitle({ enabled: true, activeCount: 2, rate: 1_048_576 }))
      .toBe('1.0 MiB/s · 2 aktiv · rDownloader')
    setByteDisplay('decimal')
    expect(transferTitle({ enabled: true, activeCount: 2, rate: 1_000_000 }))
      .toBe('1.0 MB/s · 2 aktiv · rDownloader')
  })
})
