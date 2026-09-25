import { i18n, type AppLocale } from '@/i18n'
import { withBase } from '@/basePath'

/**
 * Translations shipped inside installed plugin packages.
 *
 * The core bundle carries only generic strings; everything specific to a provider — its
 * failure codes, display name, description and credential labels — travels inside the signed
 * `.rdplug` and is merged into vue-i18n at runtime. A third-party plugin is therefore fully
 * localised without touching this repository.
 */
interface PluginMessages {
  server?: { codes?: Record<string, string> }
  providers?: Record<string, Record<string, string>>
}

/** Locales already merged, so switching back and forth does not refetch. */
const loaded = new Set<AppLocale | 'en'>()

/**
 * Locales tried and failed since the last reset.
 *
 * Kept apart from {@link loaded} on purpose. A failure must not make every language switch
 * ask again, but it must stay retryable — merging the two meant a fetch refused before
 * sign-in was remembered as "done" and the plugin strings never arrived for that session.
 */
const attempted = new Set<AppLocale | 'en'>()

/**
 * Whether a session exists that could answer a request for plugin translations.
 *
 * The endpoint requires `READ` and is deliberately not public: which plugins are installed
 * is a statement about this installation. Asking before sign-in therefore earns a `401` and
 * nothing else, so the sign-in screen does not ask at all — there are no plugin strings on
 * it to translate.
 */
let signedIn = false

async function fetchMessages(locale: string): Promise<PluginMessages | null> {
  try {
    const response = await fetch(withBase(`/api/v1/plugins/i18n/${locale}`), { credentials: 'include' })
    if (!response.ok) return null
    return (await response.json()) as PluginMessages
  } catch {
    // Offline or the backend is not reachable yet; plugin strings fall back to the
    // English text the server sends with each message.
    return null
  }
}

async function mergeLocale(locale: AppLocale): Promise<void> {
  if (loaded.has(locale) || attempted.has(locale)) return
  const messages = await fetchMessages(locale)
  if (!messages) {
    // Remembered only until the next reset, so a language switch does not ask again while
    // the answer cannot change, and signing in does not inherit the refusal.
    attempted.add(locale)
    return
  }
  loaded.add(locale)
  i18n.global.mergeLocaleMessage(locale, messages as Record<string, unknown>)
}

/**
 * Loads plugin translations for `locale`, plus English as the fallback layer.
 *
 * Safe to call repeatedly: each locale is fetched at most once per session, and a fresh
 * install invalidates it through {@link resetPluginMessages}.
 */
export async function loadPluginMessages(locale: AppLocale): Promise<void> {
  if (!signedIn) return
  await Promise.all(
    locale === 'en' ? [mergeLocale('en')] : [mergeLocale(locale), mergeLocale('en')]
  )
}

/** Forgets what was merged so the next load picks up a freshly installed plugin's strings. */
export function resetPluginMessages(): void {
  loaded.clear()
  attempted.clear()
}

/**
 * Reports whether a session exists, which is what decides if the endpoint may be asked.
 *
 * Called on every change of the authenticated state. Both directions forget what was merged:
 * signing out must not leave the previous session's plugin strings behind, and signing in has
 * to be able to fetch what the sign-in screen deliberately did not.
 */
export function setPluginMessagesAvailable(available: boolean): void {
  if (signedIn === available) return
  signedIn = available
  resetPluginMessages()
}

/**
 * Localised text for one provider, e.g. `secret_label` or `name`.
 *
 * Tries the active locale, then English: `te()` only consults the locale it is given, so a
 * plugin shipping just `en.json` would otherwise appear untranslated in every other UI
 * language. Returns `undefined` when no installed plugin supplies the field, letting callers
 * fall back to the core's generic labels.
 */
export function providerText(slug: string, key: string): string | undefined {
  const path = `providers.${slug}.${key}`
  const active = i18n.global.locale.value as AppLocale
  for (const locale of active === 'en' ? ['en'] : [active, 'en']) {
    if (i18n.global.te(path, locale)) return i18n.global.t(path, {}, { locale })
  }
  return undefined
}
