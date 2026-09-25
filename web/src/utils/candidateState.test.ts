import { describe, expect, it } from 'vitest'

import { ENQUEUEABLE_STATES, isEnqueueable, isUnverified } from './candidateState'

describe('candidate states', () => {
  it('refuses only the links the service is working on', () => {
    for (const state of ['online', 'duplicate', 'offline', 'unsupported', 'error']) {
      expect(isEnqueueable(state), state).toBe(true)
    }
    for (const state of ['resolving', 'checking', 'enqueued']) {
      expect(isEnqueueable(state), state).toBe(false)
    }
  })

  // The reported case: a hoster account whose sign-in failed left every link of the batch in
  // `error`, and the package could not be added to the downloader at all.
  it('treats a failed check no worse than a confirmed offline file', () => {
    expect(isEnqueueable('error')).toBe(isEnqueueable('offline'))
  })

  it('keeps "could not check" apart from "is gone"', () => {
    expect(isUnverified('error')).toBe(true)
    expect(isUnverified('unsupported')).toBe(true)
    expect(isUnverified('offline')).toBe(false)
  })

  // RD-110-07: the address answered with a page. That is a conclusion, not a gap, so it is
  // neither queueable nor "not checkable" — and it must never slip into either list.
  it('never offers a page that is not a file to the downloader', () => {
    expect(isEnqueueable('unresolvable')).toBe(false)
    expect(isUnverified('unresolvable')).toBe(false)
    expect(ENQUEUEABLE_STATES).not.toContain('unresolvable')
  })

  it('lists every enqueueable state exactly once', () => {
    expect(new Set(ENQUEUEABLE_STATES).size).toBe(ENQUEUEABLE_STATES.length)
  })
})
