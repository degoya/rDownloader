/**
 * Copying works, or says it did not.
 *
 * Opened over plain HTTP from another machine on the LAN the page is no secure context, and
 * `navigator.clipboard` is `undefined`: an uncaught `writeText` meant a token or an MFA code was
 * silently not copied while the card announced success.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const add = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }))

const { copyText, useCopy } = await import('./useCopy')

describe('copying to the clipboard', () => {
  const execCommand = vi.fn()

  beforeEach(() => {
    add.mockReset()
    execCommand.mockReset()
    Object.defineProperty(document, 'execCommand', { value: execCommand, configurable: true })
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  function clipboard(writeText: (text: string) => Promise<void>): void {
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
  }

  it('uses the clipboard API where the page may', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    clipboard(writeText)

    await expect(copyText('token')).resolves.toBe(true)

    expect(writeText).toHaveBeenCalledWith('token')
    expect(execCommand).not.toHaveBeenCalled()
  })

  it('copies through a selection where the clipboard API is missing', async () => {
    vi.stubGlobal('navigator', { ...navigator, clipboard: undefined })
    let selected = ''
    execCommand.mockImplementation(() => {
      selected = document.querySelector('textarea')?.value ?? ''
      return true
    })

    await expect(copyText('recovery-code')).resolves.toBe(true)

    expect(execCommand).toHaveBeenCalledWith('copy')
    expect(selected).toBe('recovery-code')
    // The helper leaves nothing behind in the page.
    expect(document.querySelector('textarea')).toBeNull()
  })

  it('falls back to the selection when the clipboard API refuses', async () => {
    clipboard(() => Promise.reject(new DOMException('denied', 'NotAllowedError')))
    execCommand.mockReturnValue(true)

    await expect(copyText('command')).resolves.toBe(true)
  })

  it('reports a copy that did not happen, with a toast', async () => {
    vi.stubGlobal('navigator', { ...navigator, clipboard: undefined })
    execCommand.mockReturnValue(false)
    const copy = useCopy()

    await expect(copy('token')).resolves.toBe(false)

    expect(add).toHaveBeenCalledWith(expect.objectContaining({ color: 'error', title: 'common.copy.failed_title' }))
  })

  it('stays quiet when the copy worked', async () => {
    clipboard(() => Promise.resolve())
    const copy = useCopy()

    await expect(copy('token')).resolves.toBe(true)

    expect(add).not.toHaveBeenCalled()
  })
})
