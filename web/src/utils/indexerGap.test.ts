import { describe, expect, it } from 'vitest'
import { INDEXER_POLL_BOUND, indexerPollGap } from './indexerGap'

const run = (found: number, archived: number, error: string | null = null) => ({
  found,
  accepted: archived,
  skipped: 0,
  error
})

describe('indexerPollGap', () => {
  it('warns when a check read to its bound and every entry was new', () => {
    expect(indexerPollGap(run(INDEXER_POLL_BOUND, INDEXER_POLL_BOUND))).toBe(true)
    expect(indexerPollGap({ found: 2000, accepted: 300, skipped: 1700, error: null })).toBe(true)
  })

  it('stays quiet when the check met an entry it already had', () => {
    // 700 new entries, the eighth page ended in the archive.
    expect(indexerPollGap(run(800, 700))).toBe(false)
    // A busy category on a normal check: one page, a few new.
    expect(indexerPollGap(run(100, 5))).toBe(false)
  })

  it('stays quiet for a first check, which reads five pages and has nothing to meet', () => {
    expect(indexerPollGap(run(500, 500))).toBe(false)
  })

  it('says nothing about a failed check or none at all', () => {
    expect(indexerPollGap(run(0, 0, 'rate limit reached'))).toBe(false)
    expect(indexerPollGap(undefined)).toBe(false)
  })
})
