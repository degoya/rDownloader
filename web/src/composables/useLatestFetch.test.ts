import { describe, expect, it } from 'vitest'

import { useLatestFetch } from './useLatestFetch'

/** A request the test answers by hand, in whatever order it likes. */
function held<T>(): { promise: Promise<T>, answer: (value: T) => void, fail: (reason: Error) => void } {
  let answer: (value: T) => void = () => {}
  let fail: (reason: Error) => void = () => {}
  const promise = new Promise<T>((resolve, reject) => {
    answer = resolve
    fail = reject
  })
  return { promise, answer, fail }
}

/** The store fetch with a ticket (WEB-06): only the newest answer lands. */
describe('the latest fetch', () => {
  it('keeps an older answer that arrives last from overwriting a newer one', async () => {
    const { run, fetching, loading } = useLatestFetch()
    const shown: string[] = []
    const older = held<string>()
    const newer = held<string>()

    expect(loading.value).toBe(true)
    const first = run(() => older.promise, value => shown.push(value))
    const second = run(() => newer.promise, value => shown.push(value))
    newer.answer('new filter')
    await expect(second).resolves.toBe(true)
    expect(fetching.value).toBe(false)
    expect(loading.value).toBe(false)

    older.answer('old filter')
    await expect(first).resolves.toBe(false)
    expect(shown).toEqual(['new filter'])
  })

  it('brings the flags down after a rejection, so a debounce waiting on them goes on', async () => {
    const { run, fetching, settled } = useLatestFetch()
    const request = held<string>()

    const pending = run(() => request.promise, () => {})
    expect(fetching.value).toBe(true)
    request.fail(new Error('offline'))

    await expect(pending).rejects.toThrow('offline')
    expect(fetching.value).toBe(false)
    expect(settled.value).toBe(true)
  })
})
