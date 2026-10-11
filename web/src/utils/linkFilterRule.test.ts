import { describe, expect, it } from 'vitest'

import type { LinkFilterRule } from '@/api/types'
import { conditionTokens, copyRequest, emptyLinkFilterForm, formFromRule, movedIds, requestFromForm } from '@/utils/linkFilterRule'

function rule(overrides: Partial<LinkFilterRule> = {}): LinkFilterRule {
  return {
    id: 'r1',
    name: 'Info files',
    position: 1,
    enabled: true,
    action: 'hide',
    name_pattern: '*.nfo',
    name_syntax: 'glob',
    size_min: null,
    size_max: null,
    extensions: [],
    hoster: null,
    source: null,
    package_name: null,
    category_id: null,
    ...overrides
  }
}

describe('linkFilterRule', () => {
  it('sends sizes in bytes, cleans file types and the hoster, and drops empty conditions', () => {
    const form = {
      ...emptyLinkFilterForm(),
      name: '  Samples ',
      namePattern: '  ',
      sizeMinMib: 1.5,
      sizeMaxMib: 0,
      extensions: ['.MKV', ' ', 'part1.rar'],
      hoster: ' Files.Example '
    }
    expect(requestFromForm(form)).toEqual({
      name: 'Samples',
      enabled: true,
      action: 'hide',
      name_pattern: null,
      name_syntax: 'glob',
      size_min: 1572864,
      size_max: null,
      extensions: ['mkv', 'part1.rar'],
      hoster: 'files.example',
      source: null,
      package_name: null,
      category_id: null
    })
  })

  it('keeps a package and a category only for a route rule', () => {
    const form = { ...emptyLinkFilterForm(), name: 'x', packageName: ' Extras ', categoryId: 'c1' }
    expect(requestFromForm(form).package_name).toBeNull()
    expect(requestFromForm({ ...form, action: 'route' })).toMatchObject({ package_name: 'Extras', category_id: 'c1' })
  })

  it('reads a stored rule back into the form it was written with', () => {
    const stored = rule({ size_min: 1572864, extensions: ['nfo'], action: 'route', package_name: 'Extras' })
    const form = formFromRule(stored)
    expect(form.sizeMinMib).toBe(1.5)
    expect(copyRequest(stored, 'Info files (copy)')).toMatchObject({
      name: 'Info files (copy)',
      size_min: 1572864,
      extensions: ['nfo'],
      package_name: 'Extras'
    })
  })

  it('moves one rule a step and refuses to move past either end', () => {
    expect(movedIds(['a', 'b', 'c'], 2, -1)).toEqual(['a', 'c', 'b'])
    expect(movedIds(['a', 'b', 'c'], 0, 1)).toEqual(['b', 'a', 'c'])
    expect(movedIds(['a', 'b', 'c'], 0, -1)).toBeNull()
    expect(movedIds(['a', 'b', 'c'], 2, 1)).toBeNull()
  })

  it('lists the conditions a rule checks', () => {
    const label = { source: (source: string) => `<${source}>`, size: (bytes: number) => `${bytes}B` }
    expect(conditionTokens(rule({ source: 'clipboard', hoster: 'files.example', extensions: ['nfo', 'txt'], size_max: 10 }), label))
      .toEqual(['<clipboard>', 'files.example', '.nfo .txt', '*.nfo', '≤ 10B'])
    expect(conditionTokens(rule({ name_pattern: 'sample', name_syntax: 'regex', size_min: 1, size_max: 2 }), label))
      .toEqual(['/sample/', '1B–2B'])
    expect(conditionTokens(rule({ name_pattern: null }), label)).toEqual([])
  })
})
