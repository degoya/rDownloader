import { ref, type Ref } from 'vue'

import { responseError } from '@/api/client'
import { useConfirm, type ConfirmOptions } from '@/composables/useConfirm'

/** What `openapi-fetch` answers with: the body on success, an error payload otherwise. */
export interface ApiResult<T> {
  data?: T | undefined
  error?: unknown
}

export interface EditableList<T, Body> {
  /** The row being edited, or `null` while the form creates a new one. */
  editingId: Ref<string | null>
  /** True while a save or a delete is in flight. */
  pending: Ref<boolean>
  /** The failure of the last save or delete, translated, or `null`. */
  error: Ref<string | null>
  /** Points the form at `row`; the caller fills its own fields and focuses them. */
  edit: (row: T) => void
  /** Leaves edit mode and clears the caller's form. */
  reset: () => void
  /** Saves the form — update when editing, create otherwise — and returns the saved row. */
  submit: (body: Body) => Promise<T | null>
  /**
   * Confirms, deletes, and drops the row.
   *
   * Answers with both parts, because they are not the same question. `removed` says whether it
   * happened; `body` is what the server said about it, which is `undefined` when the endpoint
   * answers `204 No Content`. Success is read from the absence of an error rather than from the
   * presence of a body for exactly that reason: `/api/v1/streams/channels/{id}` returns 204,
   * every other delete in this application returns a message, and a check for the body would
   * report the first as a failure.
   */
  remove: (row: T) => Promise<{ removed: boolean, body: unknown }>
}

/**
 * A list whose rows are edited in the form beside it (RD-106-15).
 *
 * Nine components carried this shape, and only the DTO and the two endpoint paths differed
 * between them: an `editingId`, a save that branches `PUT '/…/{id}'` against `POST '/…'` and then
 * either replaces the row in the local array or appends the created one, a reset, and a delete
 * that ends with `if (editingId.value === row.id) reset()`.
 *
 * That last line is the reason this is a composable and not a comment. It is a correctness rule,
 * it was re-derived nine times, and the author who forgets it leaves the form pointed at a row
 * the server no longer has — so the next save is a `PUT` to a dead id, and the reader is told the
 * request failed rather than that the thing they were editing is gone.
 *
 * The caller keeps its own form and its own `edit` mapping, because those are genuinely
 * per-component: filling a form from a row is where the DTO lives.
 */
export function useEditableList<T extends { id: string }, Body>(options: {
  /** The rows, as the caller holds them; this composable keeps them in step with the server. */
  list: Ref<T[]>
  create: (body: Body) => Promise<ApiResult<T>>
  update: (id: string, body: Body) => Promise<ApiResult<T>>
  destroy: (id: string) => Promise<ApiResult<unknown>>
  /** Clears the caller's form. Called after a save and after deleting the row being edited. */
  reset: () => void
  /** The wording of the delete confirmation for this row. */
  confirmDelete: (row: T) => ConfirmOptions
}): EditableList<T, Body> {
  const confirm = useConfirm()
  const editingId = ref<string | null>(null)
  const pending = ref(false)
  const error = ref<string | null>(null)

  function reset(): void {
    editingId.value = null
    options.reset()
  }

  function edit(row: T): void {
    error.value = null
    editingId.value = row.id
  }

  async function submit(body: Body): Promise<T | null> {
    pending.value = true
    error.value = null
    const id = editingId.value
    const response = id
      ? await options.update(id, body)
      : await options.create(body)
    pending.value = false
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    const saved = response.data
    options.list.value = id
      ? options.list.value.map(row => (row.id === saved.id ? saved : row))
      : [...options.list.value, saved]
    reset()
    return saved
  }

  async function remove(row: T): Promise<{ removed: boolean, body: unknown }> {
    if (!await confirm(options.confirmDelete(row))) return { removed: false, body: undefined }
    pending.value = true
    error.value = null
    const response = await options.destroy(row.id)
    pending.value = false
    if (response.error !== undefined) {
      error.value = responseError(response)
      return { removed: false, body: undefined }
    }
    options.list.value = options.list.value.filter(entry => entry.id !== row.id)
    // The form may be pointed at the row that just stopped existing.
    if (editingId.value === row.id) reset()
    return { removed: true, body: response.data }
  }

  return { editingId, pending, error, edit, reset, submit, remove }
}
