/** RD-1480-07: three copies of one document, field by field. */
import { describe, expect, it } from 'vitest'

import { changedPart, editedFields, mergeStored, tidied } from './settingsMerge'

const base = { max_retries: 4, media_hosts: ['a.example'], external_url: null, rules: { b: 1, a: 2 }, hotfolder_poll_seconds: 30 }

describe('the edited fields', () => {
  it('are those with another value, whatever the order of an object, and an absent one is empty', () => {
    expect(editedFields(base, { ...base, rules: { a: 2, b: 1 }, external_url: undefined })).toEqual([])
    expect(editedFields(base, { ...base, media_hosts: ['b.example', 'a.example'], rules: { a: 2, b: 2 } })).toEqual(['media_hosts', 'rules'])
  })

  it('leave out what a card of its own saves', () => {
    expect(editedFields(base, { ...base, hotfolder_poll_seconds: 60 })).toEqual([])
  })
})

describe('the part to send', () => {
  it('holds an emptied field as null and drops one that says the same once tidied', () => {
    const edited = tidied({ ...base, vendor_directory: '  ', media_hosts: ['https://www.A.example/x'], external_url: undefined, max_retries: 9 })
    expect(changedPart({ ...base, vendor_directory: '/opt' }, edited, ['vendor_directory', 'media_hosts', 'max_retries']))
      .toEqual({ vendor_directory: null, max_retries: 9 })
  })
})

describe('a stored document under the form', () => {
  it('follows what is not edited here and keeps what is', () => {
    const mine = { ...base, max_retries: 9 }
    const stored = { ...base, media_hosts: ['c.example'] }
    expect(mergeStored(base, mine, stored)).toEqual({
      merged: { ...base, max_retries: 9, media_hosts: ['c.example'] },
      foreign: ['media_hosts'],
      conflicts: []
    })
  })

  it('names the field both sides changed and keeps the unsaved value', () => {
    const { merged, foreign, conflicts } = mergeStored(base, { ...base, max_retries: 9 }, { ...base, max_retries: 5 })
    expect(merged.max_retries).toBe(9)
    expect([foreign, conflicts]).toEqual([['max_retries'], ['max_retries']])
  })

  it('is no conflict when both sides chose the same value', () => {
    expect(mergeStored(base, { ...base, max_retries: 5 }, { ...base, max_retries: 5 })).toMatchObject({ foreign: [], conflicts: [] })
  })

  it('takes the value of a self-saving card without naming it', () => {
    const { merged, foreign, conflicts } = mergeStored(base, { ...base, hotfolder_poll_seconds: 60 }, { ...base, hotfolder_poll_seconds: 90 })
    expect(merged.hotfolder_poll_seconds).toBe(90)
    expect([foreign, conflicts]).toEqual([[], []])
  })
})
