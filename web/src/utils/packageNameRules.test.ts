import { describe, expect, it } from 'vitest'

import { packageNameOverride, packageNameOverrideBody, packageNameRules } from './packageNameRules'

describe('package-name rules (RD-1140-05)', () => {
  it('reads a missing switch as off, as the service does', () => {
    expect(packageNameRules(undefined)).toEqual({ spaces_to_dots: false, collapse_separators: false, strip_bracket_tags: false, lowercase: false })
    expect(packageNameRules({ lowercase: true }).lowercase).toBe(true)
  })

  it('sends a category override only when it sets a switch', () => {
    expect(packageNameOverrideBody(null)).toBeNull()
    expect(packageNameOverrideBody({ spaces_to_dots: null, lowercase: null })).toBeNull()
    expect(packageNameOverrideBody({ lowercase: false })).toEqual({
      spaces_to_dots: null, collapse_separators: null, strip_bracket_tags: null, lowercase: false
    })
    expect(packageNameOverride(undefined).strip_bracket_tags).toBeNull()
  })
})
