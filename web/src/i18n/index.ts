import { createI18n } from 'vue-i18n'

import { DATETIME_FORMATS, NUMBER_FORMATS } from './formats'
import { messageResolver } from './resolver'
import { loadPluginMessages } from './plugins'

type LocaleMessages = Record<string, never> | { [key: string]: string | LocaleMessages }

export const SUPPORTED_LOCALES = ['en', 'de', 'fr', 'es'] as const
export type AppLocale = typeof SUPPORTED_LOCALES[number]

const STORAGE_KEY = 'rd.locale'

type MessageTree = Record<string, unknown>

/** Every `src/locales/<locale>/<domain>.json` file becomes the `<domain>` namespace. */
function loadMessages(): Record<AppLocale, MessageTree> {
  const files = import.meta.glob<{ default: MessageTree }>('../locales/*/*.json', { eager: true })
  const messages = Object.fromEntries(SUPPORTED_LOCALES.map(locale => [locale, {}])) as Record<AppLocale, MessageTree>
  for (const [path, module] of Object.entries(files)) {
    const match = /\/locales\/([a-z]{2})\/([a-z_]+)\.json$/.exec(path)
    if (!match) continue
    const [, locale, domain] = match
    if (!locale || !domain || !isSupported(locale)) continue
    messages[locale][domain] = module.default
  }
  return messages
}

export function isSupported(value: string): value is AppLocale {
  return (SUPPORTED_LOCALES as readonly string[]).includes(value)
}

/** Stored choice, else the first browser language we ship, else English. */
export function detectLocale(): AppLocale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored && isSupported(stored)) return stored
  } catch {
    // storage unavailable
  }
  const candidates = typeof navigator === 'undefined' ? [] : navigator.languages ?? [navigator.language]
  for (const candidate of candidates) {
    const primary = candidate.toLowerCase().split('-')[0] ?? ''
    if (isSupported(primary)) return primary
  }
  return 'en'
}

function perLocale<T>(value: T): Record<AppLocale, T> {
  return { en: value, de: value, fr: value, es: value }
}

export const i18n = createI18n<false>({
  legacy: false,
  globalInjection: true,
  locale: detectLocale(),
  fallbackLocale: 'en',
  messages: loadMessages() as Record<AppLocale, LocaleMessages>,
  messageResolver,
  numberFormats: perLocale(NUMBER_FORMATS),
  datetimeFormats: perLocale(DATETIME_FORMATS),
  missingWarn: false,
  fallbackWarn: false
})

/**
 * Switches the UI language, persists it and updates `<html lang>`.
 *
 * Installed plugins ship their own translations, so the new language's plugin catalogue is
 * merged in the background; until it arrives, plugin strings fall back to English. Without a
 * session that merge is skipped rather than refused — the endpoint requires one.
 */
export function setLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale
  try {
    localStorage.setItem(STORAGE_KEY, locale)
  } catch {
    // storage unavailable
  }
  if (typeof document !== 'undefined') document.documentElement.lang = locale
  void loadPluginMessages(locale)
}

export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale
}
