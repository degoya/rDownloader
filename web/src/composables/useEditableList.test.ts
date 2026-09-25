/**
 * The rules nine components used to re-derive, checked once.
 *
 * Two of them are the reason this composable exists at all: the form must stop pointing at a row
 * that has been deleted, and a delete is judged by the absence of an error rather than by the
 * presence of a response body — `/api/v1/streams/channels/{id}` answers `204 No Content`, and a
 * check for the body reports that successful delete as a failure.
 */
import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import { useEditableList } from './useEditableList'

const confirmed = vi.hoisted(() => ({ value: true }))
vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => async () => confirmed.value
}))

interface Row { id: string, name: string }

function setup(destroy: (id: string) => Promise<{ data?: unknown, error?: unknown }>) {
  const rows = ref<Row[]>([{ id: 'a', name: 'First' }, { id: 'b', name: 'Second' }])
  const reset = vi.fn()
  const list = useEditableList<Row, { name: string }>({
    list: rows,
    create: async body => ({ data: { id: 'c', name: body.name } }),
    update: async (id, body) => ({ data: { id, name: body.name } }),
    destroy,
    reset,
    confirmDelete: row => ({ title: 'Delete', description: row.name })
  })
  return { rows, reset, list }
}

const ok = async () => ({ data: { message: 'Deleted' } })
const noContent = async () => ({ data: undefined, error: undefined })

describe('useEditableList', () => {
  it('appends a created row and replaces an edited one', async () => {
    const { rows, list } = setup(ok)
    await list.submit({ name: 'Third' })
    expect(rows.value.map(row => row.id)).toEqual(['a', 'b', 'c'])

    list.edit(rows.value[0] as Row)
    await list.submit({ name: 'Renamed' })
    expect(rows.value.map(row => row.name)).toEqual(['Renamed', 'Second', 'Third'])
  })

  it('leaves edit mode once the save has landed', async () => {
    const { list, reset } = setup(ok)
    list.edit({ id: 'a', name: 'First' })
    expect(list.editingId.value).toBe('a')
    await list.submit({ name: 'First' })
    expect(list.editingId.value).toBeNull()
    expect(reset).toHaveBeenCalled()
  })

  /** The rule every one of the nine copies had to state for itself. */
  it('stops pointing the form at a row that was deleted', async () => {
    const { rows, list, reset } = setup(ok)
    list.edit(rows.value[1] as Row)
    await list.remove(rows.value[1] as Row)
    expect(list.editingId.value).toBeNull()
    expect(reset).toHaveBeenCalled()
    expect(rows.value.map(row => row.id)).toEqual(['a'])
  })

  it('keeps editing a different row when another one is deleted', async () => {
    const { rows, list } = setup(ok)
    list.edit(rows.value[0] as Row)
    await list.remove(rows.value[1] as Row)
    expect(list.editingId.value).toBe('a')
  })

  it('reads a 204 delete as done, not as a failure', async () => {
    const { rows, list } = setup(noContent)
    const { removed, body } = await list.remove(rows.value[0] as Row)
    expect(removed).toBe(true)
    expect(body).toBeUndefined()
    expect(list.error.value).toBeNull()
    expect(rows.value.map(row => row.id)).toEqual(['b'])
  })

  it('hands back the server wording when the endpoint sends one', async () => {
    const { rows, list } = setup(ok)
    const { body } = await list.remove(rows.value[0] as Row)
    expect(body).toEqual({ message: 'Deleted' })
  })

  it('keeps the row and the edit when the delete is refused', async () => {
    confirmed.value = false
    const { rows, list } = setup(ok)
    list.edit(rows.value[0] as Row)
    const { removed } = await list.remove(rows.value[0] as Row)
    confirmed.value = true
    expect(removed).toBe(false)
    expect(rows.value).toHaveLength(2)
    expect(list.editingId.value).toBe('a')
  })

  it('keeps the form filled when a save fails, so the entry is not lost', async () => {
    const rows = ref<Row[]>([])
    const reset = vi.fn()
    const list = useEditableList<Row, { name: string }>({
      list: rows,
      create: async () => ({ error: { code: 'conflict' } }),
      update: async () => ({ error: { code: 'conflict' } }),
      destroy: ok,
      reset,
      confirmDelete: () => ({ title: 'Delete', description: '' })
    })
    expect(await list.submit({ name: 'Third' })).toBeNull()
    expect(reset).not.toHaveBeenCalled()
    expect(list.error.value).not.toBeNull()
    expect(list.pending.value).toBe(false)
  })
})
