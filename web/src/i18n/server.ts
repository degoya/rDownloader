import { i18n } from '@/i18n'

/** Shape shared by API errors (`error`), action results (`message`) and download failures. */
export interface ServerMessage {
  message?: string | null
  code?: string | null
  params?: Record<string, string> | null
}

/**
 * The locales a coded message is looked up in: the active one, then English.
 *
 * `te()` consults only the locale it is given, so without the second step a plugin that
 * ships just `en.json` would show the backend's raw text in every other UI language
 * instead of its own English catalogue -- the middle rung of the documented chain.
 */
function lookupLocales(): string[] {
  const active = i18n.global.locale.value
  return active === 'en' ? ['en'] : [active, 'en']
}

/**
 * Translates a coded server message: known codes come from `server.codes.<code>` in the
 * active language, then in English (with the flat `params` interpolated); unknown codes
 * fall back to the English text the server sent, and a missing text falls back to a
 * generic error.
 */
export function translateServerMessage(value: ServerMessage | string | null | undefined): string {
  const { t, te } = i18n.global
  if (typeof value === 'string') return value
  if (!value) return t('common.errors.request_failed')
  const key = value.code ? `server.codes.${value.code}` : null
  if (key) {
    for (const locale of lookupLocales()) {
      if (!te(key, locale as never)) continue
      const params = { ...(value.params ?? {}) }
      // A `capability` parameter is a stable identifier (`media_merge`), not a phrase: the
      // backend is English-only, so the name of the thing that stopped working has to be
      // translated here or the message would say `media_merge` in every language.
      const capability = params.capability ? `server.capabilities.${params.capability}` : null
      if (capability && te(capability)) params.capability = t(capability)
      const count = Number(params.count)
      return Number.isFinite(count) && params.count !== undefined
        ? t(key, count, { named: params, locale })
        : t(key, params, { locale })
    }
  }
  return value.message || t('common.errors.request_failed')
}

/**
 * The line next to a provider account: every part translated through the chain above and
 * joined with ` · `. A part no catalogue knows and no text accompanies shows its code, so a
 * catalogue gap is visible rather than blank; a check with nothing to say yields ``.
 */
export function translateAccountLabel(parts: readonly ServerMessage[] | null | undefined): string {
  return (parts ?? [])
    .map(part => translateServerMessage({ ...part, message: part.message || part.code || null }))
    .join(' · ')
}

/** Reads `{ error, code, params }` (errors) or `{ message, code, params }` (results) from a JSON body. */
export function serverMessageFrom(body: unknown): ServerMessage | null {
  if (typeof body !== 'object' || body === null) return null
  const record = body as Record<string, unknown>
  const message = typeof record.error === 'string' ? record.error : typeof record.message === 'string' ? record.message : null
  const code = typeof record.code === 'string' ? record.code : null
  const params = typeof record.params === 'object' && record.params !== null
    ? Object.fromEntries(Object.entries(record.params as Record<string, unknown>).map(([k, v]) => [k, String(v)]))
    : null
  if (message === null && code === null) return null
  return { message, code, params }
}
