import list from '@/locales/languages.json'

/**
 * `required`: every key in every catalogue, enforced by the tests. `in-progress`: whatever is
 * translated so far — each missing key falls back to English, and the picker marks the language
 * as unfinished (RD-1100-09).
 */
type LanguageStatus = 'required' | 'in-progress'

export interface Language {
  /** Two lowercase letters: the catalogue directory, the plugin file name and `<html lang>`. */
  code: string
  /** The language's own name for itself, as the picker shows it in every UI language. */
  name: string
  status: LanguageStatus
}

/**
 * `web/src/locales/languages.json`, the one list of languages (RD-1100-09).
 *
 * The web interface, the extension's tests, the plugin catalogue checks and
 * `scripts/i18n-key.sh` all read it, so a new language is one entry there plus its catalogues —
 * no code. The order is the picker's order.
 */
export const LANGUAGES: readonly Language[] = Object.entries(list as Record<string, { name: string, status: string }>)
  .map(([code, entry]) => ({ code, name: entry.name, status: entry.status as LanguageStatus }))

export const SUPPORTED_LOCALES: readonly string[] = LANGUAGES.map(language => language.code)

/** The languages the build refuses to ship incomplete; English is always one of them. */
export const REQUIRED_LOCALES: readonly string[] = LANGUAGES
  .filter(language => language.status === 'required')
  .map(language => language.code)

export function isInProgress(code: string): boolean {
  return LANGUAGES.some(language => language.code === code && language.status === 'in-progress')
}

type Translate = (key: string, named: Record<string, unknown>) => string

/** The language picker's entries, an unfinished language marked in the reader's language. */
export function languageItems(t: Translate): { label: string, value: string }[] {
  return LANGUAGES.map(({ code, name, status }) => ({
    label: status === 'in-progress' ? t('common.preferences.language_in_progress', { name }) : name,
    value: code
  }))
}
