import { describe, expect, it } from 'vitest'

import { NO_SELECTION, optionalSelection, selectionValue } from './select'

describe('optional select values', () => {
  it('uses a non-empty sentinel for an unset selection', () => {
    expect(optionalSelection(null)).toBe(NO_SELECTION)
    expect(NO_SELECTION).not.toBe('')
    expect(selectionValue(NO_SELECTION)).toBeNull()
  })
})
