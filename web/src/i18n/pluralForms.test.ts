/**
 * A counted sentence has both forms (RD-1480-15): "1 sessions were ended" was one of about fifty.
 *
 * A key whose caller gives a number picks its form; a sentence with two counts, or one whose count
 * never reaches the plural, says "sessions ended: 1" instead.
 */
import { beforeAll, describe, expect, it } from 'vitest'

import { i18n } from '@/i18n'
import { translateServerMessage } from '@/i18n/server'
import { REQUIRED_LOCALES, catalogueStrings } from '@/test/catalogues'
import { loadEveryLocale } from '@/test/locales'

beforeAll(loadEveryLocale)

/** Only ever shown for two or more. */
const ALWAYS_SEVERAL: readonly string[] = ['downloads:traffic.many', 'linkgrabber:mirror.all_offline_hint']

it('counts with a plural in English', () => {
  const offenders = [...catalogueStrings('en')].filter(([key, text]) =>
    !ALWAYS_SEVERAL.includes(key) && !text.includes('|') && /\{count\} (?:[\w-]+ ){0,2}[a-z]+s\b/u.test(text))
  expect(offenders).toEqual([])
})

describe.each(REQUIRED_LOCALES)('%s plurals', (locale) => {
  it('never says both in brackets', () => {
    const offenders = [...catalogueStrings(locale)].filter(([, text]) => /\p{L}\((?:en|e|n|s|es)\)/u.test(text.replaceAll('http(s)', '')))
    expect(offenders).toEqual([])
  })

  it('has as many forms as English', () => {
    const english = catalogueStrings('en')
    const forms = (text: string | undefined): number => (text ?? '').split('|').length
    const offenders = [...catalogueStrings(locale)].filter(([key, text]) => forms(text) !== forms(english.get(key)))
    expect(offenders).toEqual([])
  })

  it('picks the form of the number', () => {
    i18n.global.locale.value = locale as 'en'
    const { t } = i18n.global
    const keys = ['system.data_reset.done', 'system.mfa.remaining', 'system.sessions.revoke_others.description', 'automation.action_count', 'siterules.clear.done']
    const same = keys.filter(key => t(key, { count: 1 }).replace('1', '2') === t(key, { count: 2 }))
    // A formatted count is text: the third parameter decides.
    const formatted = t('settings.cleanup.rows', { count: '1' }, 1).replace('1', '2') === t('settings.cleanup.rows', { count: '2' }, 2)
    const server = ['session.others_revoked', 'backup.restore_root_missing', 'update.transfers_active', 'package.files_remove_failed'].filter((code) => {
      const text = (count: string): string => translateServerMessage({ code, message: code, params: { count, detail: 'x' } })
      return text('1').replace('1', '2') === text('2') || text('1').includes('|')
    })
    i18n.global.locale.value = 'en'
    expect({ same, formatted, server }).toEqual({ same: [], formatted: false, server: [] })
  })
})
