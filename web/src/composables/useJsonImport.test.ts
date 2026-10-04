import { describe, expect, it, vi } from 'vitest'

import { JsonRefusal, useJsonImport } from './useJsonImport'

/** A `change` event carrying one file, as the hidden input fires it. */
function changeWith(text: string, name = 'bundle.json'): Event {
  const input = document.createElement('input')
  input.type = 'file'
  const file = new File([text], name, { type: 'application/json' })
  Object.defineProperty(input, 'files', { value: { item: () => file, length: 1 } })
  const event = new Event('change')
  Object.defineProperty(event, 'target', { value: input })
  return event
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

    await jsonImport.select(changeWith('{"format":"bundle"}', 'routing.json'))

    expect(refuse).not.toHaveBeenCalled()
    expect(take).toHaveBeenCalledWith({ format: 'bundle' }, expect.objectContaining({ name: 'routing.json' }))
  })

  it('says why a file is refused, and that a file is no JSON at all', async () => {
    const { refuse, take, jsonImport } = setup()

    await jsonImport.select(changeWith('{"format":"other"}'))
    await jsonImport.select(changeWith('{ not json'))

    expect(refuse.mock.calls).toEqual([['not a bundle'], ['unreadable']])
    expect(take).not.toHaveBeenCalled()
  })

  it('opens the picker cleared, so the same file can be chosen twice', () => {
    const { jsonImport } = setup()
    const input = document.createElement('input')
    input.type = 'file'
    const click = vi.spyOn(input, 'click').mockImplementation(() => {})
    jsonImport.fileInput.value = input

    jsonImport.choose()

    expect(input.value).toBe('')
    expect(click).toHaveBeenCalledTimes(1)
  })
})
