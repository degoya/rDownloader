import { describe, expect, it } from 'vitest'

import type { AutomationVersion } from '@/api/types'

import { changedParts, describeAction, describeCondition } from './automationText'

const t = (key: string) => `<${key}>`

function version(number: number, part: Partial<AutomationVersion>): AutomationVersion {
  return {
    id: `v${number}`,
    automation_id: 'a1',
    version: number,
    trigger: 'download_completed',
    condition: { type: 'always' },
    actions: [{ kind: 'pause_package' }],
    created_at: '2026-10-02T10:00:00Z',
    ...part
  }
}

describe('describeCondition', () => {
  it('reads a nested tree in reading order', () => {
    expect(describeCondition({
      type: 'all',
      nodes: [
        { type: 'predicate', predicate: { field: 'name', operator: 'contains', value: 'x' } },
        { type: 'not', node: { type: 'predicate', predicate: { field: 'extension', operator: 'equals', value: 'nfo' } } }
      ]
    }, t)).toBe('<automation.condition.all> (<automation.field.name> <automation.operator.contains> "x"; '
      + '<automation.condition.not> (<automation.field.extension> <automation.operator.equals> "nfo"))')
    expect(describeCondition({ type: 'always' }, t)).toBe('<automation.condition.always>')
  })
})

describe('describeAction', () => {
  it('names what an action points at, or its id when that is gone', () => {
    const names = (id: string) => (id === 'c1' ? 'Movies' : undefined)
    expect(describeAction({ kind: 'set_category', category_id: 'c1' }, t, names)).toBe('<automation.action.set_category>: Movies')
    expect(describeAction({ kind: 'webhook', target_id: 'gone' }, t, names)).toBe('<automation.action.webhook>: gone')
    expect(describeAction({ kind: 'script', name: 'tidy.sh' }, t, names)).toBe('<automation.action.script>: tidy.sh')
    expect(describeAction({ kind: 'pause_package' }, t, names)).toBe('<automation.action.pause_package>')
  })
})

describe('changedParts', () => {
  it('marks what a version changed against the one before it', () => {
    const first = version(1, {})
    const second = version(2, { actions: [{ kind: 'resume_package' }] })
    expect([...changedParts(first, undefined)]).toEqual([])
    expect([...changedParts(second, first)]).toEqual(['actions'])
    expect([...changedParts(version(3, { trigger: 'package_failed', condition: { type: 'not', node: { type: 'always' } } }), second)])
      .toEqual(['trigger', 'condition', 'actions'])
  })
})
