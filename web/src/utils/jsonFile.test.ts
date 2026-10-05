import { afterEach, describe, expect, it, vi } from 'vitest'

import { downloadJson } from './jsonFile'

describe('the JSON file helpers', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('offers the data as a dated, readable JSON download and leaves nothing behind', async () => {
    vi.useFakeTimers({ now: new Date('2026-09-30T12:00:00Z'), toFake: ['Date'] })
    const blobs: Blob[] = []
    URL.createObjectURL = vi.fn((blob: Blob) => { blobs.push(blob); return 'blob:x' })
    URL.revokeObjectURL = vi.fn()
    const clicked: HTMLAnchorElement[] = []
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) { clicked.push(this) })

    downloadJson({ format: 'x', version: 1 }, 'site-rules')

    expect(clicked[0]?.download).toBe('rdownloader-site-rules-2026-09-30.json')
    expect(await blobs[0]?.text()).toBe('{\n  "format": "x",\n  "version": 1\n}')
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:x')
    expect(document.querySelector('a')).toBeNull()
  })
})
