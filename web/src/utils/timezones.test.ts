import { afterEach, describe, expect, it, vi } from 'vitest'

import { browserTimezone, timezoneOptions } from './timezones'

describe('timezoneOptions', () => {
  afterEach(() => { vi.unstubAllGlobals() })

  it('offers the browser list when it is available', () => {
    const options = timezoneOptions()
    expect(options).toContain('Europe/Berlin')
    expect(options).toContain('UTC')
  })

  it('is sorted, so a long list can be scanned', () => {
    const options = timezoneOptions()
    expect(options).toEqual([...options].sort((a, b) => a.localeCompare(b)))
  })

  it('keeps a stored zone the browser does not know', () => {
    // A schedule saved elsewhere must not appear blank or silently change zone.
    expect(timezoneOptions('Mars/Olympus')).toContain('Mars/Olympus')
  })

  it('falls back to a usable list when the browser has no supportedValuesOf', () => {
    vi.stubGlobal('Intl', { DateTimeFormat: Intl.DateTimeFormat })

    const options = timezoneOptions()

    expect(options).toContain('UTC')
    expect(options).toContain('Europe/Berlin')
  })

  it('reports UTC rather than throwing when the browser zone is unavailable', () => {
    vi.stubGlobal('Intl', {
      DateTimeFormat: () => { throw new Error('no Intl') }
    })

    expect(browserTimezone()).toBe('UTC')
  })
})
