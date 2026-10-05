import { beforeEach, describe, expect, it, vi } from 'vitest'

import { useErrorToast } from './useErrorToast'

const add = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))

beforeEach(() => add.mockReset())

describe('useErrorToast', () => {
  it('shows the title in the error colour with the alert icon', () => {
    useErrorToast()('Import failed')
    expect(add).toHaveBeenCalledWith({ title: 'Import failed', color: 'error', icon: 'i-lucide-circle-alert' })
  })

  it('carries a description when there is one', () => {
    useErrorToast()('Import failed', 'The file is not a torrent')
    expect(add).toHaveBeenCalledWith({
      title: 'Import failed',
      description: 'The file is not a torrent',
      color: 'error',
      icon: 'i-lucide-circle-alert'
    })
  })

  it('leaves an empty or missing description out instead of showing an empty line', () => {
    const showError = useErrorToast()
    showError('A', '')
    showError('B', null)
    showError('C', undefined)
    for (const [toast] of add.mock.calls) expect(toast).not.toHaveProperty('description')
  })
})
