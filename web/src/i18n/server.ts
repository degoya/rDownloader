import { i18n } from '@/i18n'
import { messageResolver } from '@/i18n/resolver'

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
 * Whether `key`'s line in `locale` names a parameter `params` does not carry.
 *
 * vue-i18n renders a missing parameter as nothing, so such a line has a hole where the cause
 * belongs. A LinkGrabber row stores a failure's code and English text but not its parameters,
 * and a yt-dlp failure read "yt-dlp fehlgeschlagen: " with nothing after the colon
 * (RD-1240-37).
 */
function lacksParams(key: string, locale: string, params: Record<string, string>, counted: boolean): boolean {
  const line = messageResolver(i18n.global.getLocaleMessage(locale as never), key)
  if (typeof line !== 'string') return false
  return [...line.matchAll(/\{\s*([A-Za-z_]\w*)\s*\}/g)]
    .map(match => match[1] ?? '')
    .some(name => params[name] === undefined && !(counted && name === 'n'))
}

/**
 * Translates a coded server message: known codes come from `server.codes.<code>` in the
 * active language, then in English (with the flat `params` interpolated); unknown codes
 * fall back to the English text the server sent, and a missing text falls back to a
 * generic error. A known code whose line needs a parameter the message did not bring also
 * shows the server's text, which carries it, rather than a sentence with a hole.
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
      const counted = Number.isFinite(count) && params.count !== undefined
      if (value.message && value.message !== value.code && lacksParams(key, locale, params, counted)) {
        return value.message
      }
      return counted
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
