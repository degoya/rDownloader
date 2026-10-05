import { describe, expect, it, vi } from 'vitest'

import { JsonRefusal, useJsonImport } from './useJsonImport'

/** One file, as the `UFileUpload` reports it. */
function fileWith(text: string, name = 'bundle.json'): File {
  return new File([text], name, { type: 'application/json' })
}

/** The JSON import the four backup surfaces share (WEB-09). */
describe('the JSON import', () => {
  function setup() {
    const refuse = vi.fn()
    const take = vi.fn()
    const jsonImport = useJsonImport<{ format: string }>({
      check: parsed => (parsed as { format?: string }).format === 'bundle'
        ? parsed as { format: string }
        : new JsonRefusal('not a bundle'),
      unreadable: () => 'unreadable',
      refuse,
      take
    })
    return { refuse, take, jsonImport }
  }

  it('hands an accepted file over with its name', async () => {
    const { refuse, take, jsonImport } = setup()

    await jsonImport.select(fileWith('{"format":"bundle"}', 'routing.json'))

    expect(refuse).not.toHaveBeenCalled()
    expect(take).toHaveBeenCalledWith({ format: 'bundle' }, expect.objectContaining({ name: 'routing.json' }))
  })

  it('says why a file is refused, and that a file is no JSON at all', async () => {
    const { refuse, take, jsonImport } = setup()

    await jsonImport.select(fileWith('{"format":"other"}'))
    await jsonImport.select(fileWith('{ not json'))

    expect(refuse.mock.calls).toEqual([['not a bundle'], ['unreadable']])
    expect(take).not.toHaveBeenCalled()
  })

  it('does nothing when the field reports no file', async () => {
    const { refuse, take, jsonImport } = setup()

    await jsonImport.select(null)

    expect(refuse).not.toHaveBeenCalled()
    expect(take).not.toHaveBeenCalled()
  })
})
