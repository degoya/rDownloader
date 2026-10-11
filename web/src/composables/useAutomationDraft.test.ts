import { describe, expect, it } from 'vitest'

import { actionComplete, linksOf, MAX_ACTION_LINKS, toDraft, toWire } from './useAutomationDraft'

describe('the automation draft (RD-1240-10)', () => {
  it('keeps an add_links text area as typed and sends the lines as links', () => {
    // Typing Enter must not lose the new line, so the draft holds the text, not the list.
    const draft = toDraft({ kind: 'add_links', links: ['https://a.example/1', 'https://a.example/2'], destination: 'downloads' })
    expect(draft).toEqual({ kind: 'add_links', links_text: 'https://a.example/1\nhttps://a.example/2', destination: 'downloads' })
    expect(toWire({ ...draft, links_text: ' https://a.example/1 \n\nhttps://a.example/3\n' })).toEqual({
      kind: 'add_links',
      links: ['https://a.example/1', 'https://a.example/3'],
      destination: 'downloads'
    })
    expect(linksOf(undefined)).toEqual([])
  })

  it('passes every other action through unchanged', () => {
    const notify = { kind: 'notify' as const, target_id: 't1', message: 'Started' }
    expect(toWire(toDraft(notify))).toEqual(notify)
    expect(toWire(toDraft({ kind: 'set_priority', priority: 'low' }))).toEqual({ kind: 'set_priority', priority: 'low' })
  })

  it('saves an action only once it names everything the server needs', () => {
    expect(actionComplete({ kind: 'start_queue' })).toBe(true)
    expect(actionComplete({ kind: 'set_priority' })).toBe(false)
    expect(actionComplete({ kind: 'notify', target_id: 't1', message: '  ' })).toBe(false)
    expect(actionComplete({ kind: 'notify', target_id: 't1', message: 'Started' })).toBe(true)
    expect(actionComplete({ kind: 'notify', message: 'Started' })).toBe(false)
    expect(actionComplete({ kind: 'add_links', links_text: '\n' })).toBe(false)
    expect(actionComplete({ kind: 'add_links', links_text: 'https://a.example/1' })).toBe(true)
    const tooMany = Array.from({ length: MAX_ACTION_LINKS + 1 }, (_, index) => `https://a.example/${index}`).join('\n')
    expect(actionComplete({ kind: 'add_links', links_text: tooMany })).toBe(false)
  })
})
