/**
 * A menu path in a sentence names the pages of today (RD-1480-15).
 *
 * "Settings → General" outlived the page it named, in four languages at once: the labels moved
 * and the sentences that point at them did not. Every `Settings → …` is read against the labels
 * of the settings pages and their tabs, so renaming one names the sentences to change. One arrow
 * (`→`) for all of them, or a path could hide from this test behind another sign.
 */
import { describe, expect, it } from 'vitest'

import { SETTINGS_SECTIONS, SETTINGS_SUB_TABS, type SettingsSubTab } from '@/settingsSections'
import { REQUIRED_LOCALES, catalogueString, catalogueStrings } from '@/test/catalogues'

/** Longest first, so "System" never answers for a longer label that starts with it. */
const byLength = <T extends { label: string }>(labels: T[]): T[] => labels.sort((a, b) => b.label.length - a.label.length)

const startsWithLabel = (text: string, label: string): boolean =>
  text.startsWith(label) && !/^[\p{L}\p{N}]/u.test(text.slice(label.length))

/** What is wrong with the path after "Settings → ", or nothing. */
function fault(path: string, strings: Map<string, string>): string | null {
  const label = (key: string): string => catalogueString(strings, key.replace('.', ':'))
  const page = byLength(SETTINGS_SECTIONS.map(section => ({ value: section.value, label: label(section.labelKey) })))
    .find(section => startsWithLabel(path, section.label))
  if (!page) return `no settings page: ${path}`
  const rest = path.slice(page.label.length)
  if (!rest.startsWith(' → ')) return null
  const tabs: readonly SettingsSubTab[] = (SETTINGS_SUB_TABS as Record<string, readonly SettingsSubTab[]>)[page.value] ?? []
  const tab = byLength(tabs.map(entry => ({ label: label(entry.labelKey) }))).find(entry => startsWithLabel(rest.slice(3), entry.label))
  return tab ? null : `no tab of ${page.label}: ${path}`
}

describe.each(REQUIRED_LOCALES)('%s menu paths', (locale) => {
  const strings = catalogueStrings(locale)
  const settings = catalogueString(strings, 'nav:settings')

  it('name a settings page and its tab', () => {
    const paths = [...strings].flatMap(([key, text]) =>
      text.split(`${settings} → `).slice(1).map(path => ({ key, path })))
    expect(paths.length).toBeGreaterThan(15)
    expect(paths.flatMap(({ key, path }) => {
      const found = fault(path, strings)
      return found ? [`${key}: ${found}`] : []
    })).toEqual([])
  })

  it('use one arrow and start at the settings', () => {
    const offenders = [...strings].filter(([, text]) =>
      text.includes('›') || SETTINGS_SECTIONS.some((section) => {
        const label = catalogueString(strings, section.labelKey.replace('.', ':'))
        return text.includes(`${settings}, ${label}`) || (text.includes(`(${label} → `))
      }))
    expect(offenders).toEqual([])
  })

  it('the tour names the pages it describes by their labels', () => {
    const tour = catalogueString(strings, 'tour:steps.settings.description')
    const missing = ['routing', 'accounts', 'usenet', 'postprocess', 'clients', 'backup']
      .map(page => catalogueString(strings, `settings:tabs.${page}`))
      .filter(label => !tour.includes(label))
    expect(missing).toEqual([])
  })
})

it('finds a path to a page or tab that does not exist', () => {
  const strings = catalogueStrings('en')
  expect(fault('General. Then', strings)).not.toBeNull()
  expect(fault('Accounts → Authentication profiles', strings)).not.toBeNull()
  expect(fault('Accounts → Site logins.', strings)).toBeNull()
  expect(fault('System — and', strings)).toBeNull()
})
