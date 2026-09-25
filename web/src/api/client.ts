import createClient from 'openapi-fetch'

import { serverMessageFrom, translateServerMessage } from '@/i18n/server'

import { BASE_PATH } from '@/basePath'

import type { paths } from './schema'

// Every generated path starts with `/api/v1`, so the mount point goes in front of it here
// rather than at 200-odd call sites.
export const api = createClient<paths>({
  baseUrl: BASE_PATH,
  credentials: 'include'
})

/** The code the service answers with when a request needs a session and has none. */
export const SESSION_REQUIRED = 'auth.session_required'

const sessionLostListeners = new Set<() => void>()

/**
 * Calls `listener` whenever the service refuses a request because the session behind it is
 * gone — ended by the idle limit or the maximum lifetime (RD-130-09), or signed out from
 * another browser. Returns the function that stops listening.
 *
 * Keyed on the code, not on the status: a wrong password at the sign-in is a `401` as well,
 * and treating that as a lost session would answer it with "your sign-in has expired".
 */
export function onSessionLost(listener: () => void): () => void {
  sessionLostListeners.add(listener)
  return () => sessionLostListeners.delete(listener)
}

/**
 * The one place a lapsed session is noticed. Without it every view met the `401` on its own
 * and most of them showed a generic error or nothing at all, so a session that ran out in the
 * middle of the evening looked like a service that had stopped working.
 */
export async function noticeLostSession(response: Response): Promise<void> {
  if (response.status !== 401 || sessionLostListeners.size === 0) return
  const body: unknown = await response.clone().json().catch(() => null)
  if (isRecord(body) && body.code === SESSION_REQUIRED) {
    for (const listener of sessionLostListeners) listener()
  }
}

api.use({ onResponse: ({ response }) => noticeLostSession(response) })

/** Turns an error payload (`{ error, code, params }`, a string or unknown) into translated text. */
export function errorMessage(error: unknown): string {
  if (typeof error === 'string') {
    return error
  }
  return translateServerMessage(serverMessageFrom(error))
}

/** Translated message of an `openapi-fetch` response that carries `error`. */
export function responseError(response: unknown): string {
  if (isRecord(response) && 'error' in response) {
    return errorMessage(response.error)
  }
  return errorMessage(undefined)
}

/** Translated text of a successful `MessageResponse`. */
export function resultMessage(body: unknown): string {
  return translateServerMessage(serverMessageFrom(body))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}
