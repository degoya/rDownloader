/**
 * The coded call of the modules that address their routes by hand — storage, the plugin
 * repositories, the bundled plugins and the update check.
 *
 * Their callers need the refusal as a coded message with its parameters (a `409` that asks for a
 * key to be confirmed carries the fingerprint), not the translated text `responseError` makes.
 * There were two copies of this helper, both on plain `fetch`, so neither ran the client's
 * middleware: a lapsed session went unnoticed, the connection dot stayed green and a dropped
 * connection was a bare failure without `network.unreachable` (WEB-03, WEB-04). It goes through
 * the generated client's `request` now, which runs the middleware like every other request.
 */
import { serverMessageFrom, type ServerMessage } from '@/i18n/server'

import { api } from './client'
import type { paths } from './schema'

/** The body of a successful answer, or the coded message of a refusal (`null` without one). */
export type Answer<T> =
  | { ok: true, data: T }
  | { ok: false, status: number, message: ServerMessage | null }

type CallMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'

/** A schema path with every `{parameter}` open to its value: `/categories/{id}` takes `/categories/${string}`. */
type Filled<P extends string> = P extends `${infer Head}{${string}}${infer Tail}` ? `${Head}${string}${Filled<Tail>}` : P

/**
 * Every path the generated schema knows, with or without a query string, so a mistyped or
 * removed route fails `vue-tsc` instead of answering `404` at run time (RA-WEB-06).
 */
export type CallPath = Filled<keyof paths & string> | `${Filled<keyof paths & string>}?${string}`

/** The client's `request` with the path checked by `CallPath` and the body by the callers' own types. */
type LooseRequest = (method: CallMethod, path: string, init: object) =>
  Promise<{ data?: unknown, error?: unknown, response: Response }>

/**
 * Sends `body` as JSON, or a `Blob` as it is (a `.rdplug` package for the preview and the upload
 * install). A request that never reached the service comes back from the middleware as a `503`
 * with its code, so only a thrown abort or an unreadable success lands in the `catch`.
 */
export async function call<T>(method: CallMethod, path: CallPath, body?: Blob | object): Promise<Answer<T>> {
  const init = body instanceof Blob
    ? { body, bodySerializer: () => body, headers: { 'Content-Type': 'application/octet-stream' } }
    : body === undefined ? {} : { body }
  try {
    const { data, error, response } = await (api.request as unknown as LooseRequest)(method, path, init)
    if (response.ok) return { ok: true, data: data as T }
    return { ok: false, status: response.status, message: serverMessageFrom(error) }
  } catch {
    return { ok: false, status: 0, message: null }
  }
}
