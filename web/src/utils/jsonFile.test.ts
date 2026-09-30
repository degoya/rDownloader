import { afterEach, describe, expect, it, vi } from 'vitest'

import { chosenFile, downloadJson, openFilePicker } from './jsonFile'

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

  it('clears the file input before opening it, so the same file fires again', () => {
    const input = document.createElement('input')
    input.type = 'file'
    const click = vi.spyOn(input, 'click').mockImplementation(() => {})

    openFilePicker(input)
    openFilePicker(null)

    expect(input.value).toBe('')
    expect(click).toHaveBeenCalledOnce()
  })

  it('reads the chosen file from a change event, and nothing from anything else', () => {
    const file = new File(['{}'], 'bundle.json')
    const input = document.createElement('input')
    input.type = 'file'
    Object.defineProperty(input, 'files', { value: { item: () => file } })
    const change = new Event('change')
    Object.defineProperty(change, 'target', { value: input })

    expect(chosenFile(change)).toBe(file)
    expect(chosenFile(new Event('change'))).toBeNull()
  })
})
