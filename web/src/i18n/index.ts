import { createI18n } from 'vue-i18n'

import { DATETIME_FORMATS, NUMBER_FORMATS } from './formats'
import { REQUIRED_LOCALES, SUPPORTED_LOCALES } from './languages'
import { messageResolver } from './resolver'
import { loadPluginMessages } from './plugins'

export { LANGUAGES, REQUIRED_LOCALES, SUPPORTED_LOCALES, isInProgress, languageItems } from './languages'

type LocaleMessages = Record<string, never> | { [key: string]: string | LocaleMessages }

/** A code from `web/src/locales/languages.json`; the list is data, so the type is a string. */
export type AppLocale = string

const STORAGE_KEY = 'rd.locale'

type MessageTree = Record<string, unknown>

/**
 * Every `src/locales/<locale>/<domain>.json` file becomes the `<domain>` namespace.
 *
 * Only English — the fallback every other language leans on — is bundled into the main chunk;
 * every other language is a separate chunk fetched the first time it is needed (RD-140-27). All
 * four in the main chunk were 1.3 MB of JSON nobody but one language's reader ever used. A key a
 * language does not have yet resolves through `fallbackLocale`, so an unfinished language shows
 * English for it, never the raw key (RD-1100-09).
 */
const englishFiles = import.meta.glob<{ default: MessageTree }>('../locales/en/*.json', { eager: true })
const lazyFiles = import.meta.glob<{ default: MessageTree }>(['../locales/*/*.json', '!../locales/en/*.json'])

function collect(entries: [string, MessageTree][]): Partial<Record<AppLocale, MessageTree>> {
  const messages: Partial<Record<AppLocale, MessageTree>> = {}
  for (const [path, tree] of entries) {
    const match = /\/locales\/([a-z]{2})\/([a-z_]+)\.json$/.exec(path)
    if (!match) continue
    const [, locale, domain] = match
    if (!locale || !domain || !isSupported(locale)) continue
    ;(messages[locale] ??= {})[domain] = tree
  }
  return messages
}

/** Locales whose bundled catalogues are in vue-i18n; English is there from the start. */
const loadedLocales = new Set<AppLocale>(['en'])

/**
 * Brings `locale`'s bundled catalogues into vue-i18n, fetching its chunks on first use.
 *
 * Merged rather than set, so plugin translations that arrived first are not overwritten.
 */
export async function loadLocaleMessages(locale: AppLocale): Promise<void> {
  if (loadedLocales.has(locale)) return
  const prefix = `../locales/${locale}/`
  const entries = await Promise.all(
    Object.entries(lazyFiles)
      .filter(([path]) => path.startsWith(prefix))
      .map(async ([path, load]) => [path, (await load()).default] as [string, MessageTree])
  )
  const tree = collect(entries)[locale] ?? {}
  i18n.global.mergeLocaleMessage(locale, tree as Record<string, unknown>)
  loadedLocales.add(locale)
}

function isSupported(value: string): value is AppLocale {
  return SUPPORTED_LOCALES.includes(value)
}

/**
 * Stored choice, else the first browser language we ship complete, else English.
 *
 * An unfinished language is offered in the picker but never chosen for the reader: a page half
 * in English is a choice to make, not one to be handed.
 */
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
    if (REQUIRED_LOCALES.includes(primary)) return primary
  }
  return 'en'
}

function perLocale<T>(value: T): Record<AppLocale, T> {
  return Object.fromEntries(SUPPORTED_LOCALES.map(locale => [locale, value]))
}

export const i18n = createI18n<false>({
  legacy: false,
  globalInjection: true,
  // English until `setLocale(detectLocale())` has the detected language's chunk (see main.ts).
  locale: 'en',
  fallbackLocale: 'en',
  messages: { en: (collect(Object.entries(englishFiles).map(([path, module]) => [path, module.default])).en ?? {}) as LocaleMessages },
  messageResolver,
  numberFormats: perLocale(NUMBER_FORMATS),
  datetimeFormats: perLocale(DATETIME_FORMATS),
  missingWarn: false,
  fallbackWarn: false
})

/** The language asked for last, so a slow chunk cannot overturn a later choice. */
let requestedLocale: AppLocale | null = null

function applyLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale
  try {
    localStorage.setItem(STORAGE_KEY, locale)
  } catch {
    // storage unavailable
  }
  if (typeof document !== 'undefined') document.documentElement.lang = locale
  void loadPluginMessages(locale)
}

/**
 * Switches the UI language, persists it and updates `<html lang>`.
 *
 * A language whose catalogues are not loaded yet is switched to once its chunk has arrived, so
 * the page never shows English for a moment in between; one already loaded switches at once.
 * A chunk that cannot be fetched leaves the current language in place.
 *
 * Installed plugins ship their own translations, so the new language's plugin catalogue is
 * merged in the background; until it arrives, plugin strings fall back to English. Without a
 * session that merge is skipped rather than refused — the endpoint requires one.
 */
export async function setLocale(locale: AppLocale): Promise<void> {
  if (!isSupported(locale)) return
  requestedLocale = locale
  if (!loadedLocales.has(locale)) {
    try {
      await loadLocaleMessages(locale)
    } catch {
      // Offline or a stale deployment without that chunk: stay in the current language.
      return
    }
    if (requestedLocale !== locale) return
  }
  applyLocale(locale)
}

export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale
}
