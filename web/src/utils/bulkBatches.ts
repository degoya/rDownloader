import { responseError } from '@/api/client'
import { i18n } from '@/i18n'
import { serverMessageFrom, translateServerMessage, type ServerMessage } from '@/i18n/server'

/**
 * The most ids one bulk request may carry: every bulk route refuses more (`MAX_BULK` in
 * `rd_api_core::list_bounds`, audit 1.9.1), so a request body stays bounded however much is selected.
 */
export const BULK_LIMIT = 500

/** What a run of batches brought back. */
interface BatchRun<D> {
  /** The answers of the batches that went through, in order. */
  data: D[]
  /** The ids of those batches, in the same order. */
  sent: string[][]
  /** The response that stopped the run, or `null` when every batch went through. */
  failure: unknown
  /** How many batches the ids were split into. */
  total: number
}

/** Splits `ids` into consecutive slices of at most `size`. */
export function batches(ids: readonly string[], size = BULK_LIMIT): string[][] {
  const slices: string[][] = []
  for (let start = 0; start < ids.length; start += size) slices.push(ids.slice(start, start + size))
  return slices
}

/**
 * Sends `ids` to a bulk route in batches of at most {@link BULK_LIMIT}, one after the other.
 *
 * "Select all" over 733 files used to be one request the server refused whole, so nothing
 * happened at all. The batches run in order and the first response without `data` stops the
 * run: a refusal of the second batch is not a reason to try the third, and the caller reports
 * what the earlier ones already did.
 */
export async function inBatches<D>(
  ids: readonly string[],
  send: (batch: string[]) => Promise<{ data?: D | undefined }>
): Promise<BatchRun<D>> {
  const slices = batches(ids)
  const run: BatchRun<D> = { data: [], sent: [], failure: null, total: slices.length }
  for (const batch of slices) {
    const response = await send(batch)
    if (response.data === undefined) {
      run.failure = response
      break
    }
    run.data.push(response.data)
    run.sent.push(batch)
  }
  return run
}

/**
 * The error a stopped run shows: the response's own message, and once batches went through
 * before it, how many — so a partial success does not read as "nothing happened".
 */
export function batchError(run: BatchRun<unknown>): string | null {
  if (!run.failure) return null
  const error = responseError(run.failure)
  return run.sent.length
    ? i18n.global.t('common.errors.batch_stopped', { error, done: run.sent.length, total: run.total })
    : error
}

/**
 * One translated message for the `MessageResponse`s of several batches: their `count`s added
 * up when every one carries a count, otherwise the last batch's message.
 */
export function combinedMessage(bodies: readonly unknown[]): string | null {
  const messages = bodies.map(serverMessageFrom).filter((message): message is ServerMessage => message !== null)
  const last = messages.at(-1)
  if (!last) return null
  const counts = messages.map(message => Number(message.params?.count))
  if (messages.length < 2 || counts.some(count => !Number.isFinite(count))) return translateServerMessage(last)
  const count = counts.reduce((sum, value) => sum + value, 0)
  return translateServerMessage({ ...last, params: { ...last.params, count: String(count) } })
}
