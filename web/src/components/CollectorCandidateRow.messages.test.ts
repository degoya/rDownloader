import { render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { LinkCandidate } from '@/api/types'
import { i18n as appI18n } from '@/i18n'
import common from '@/locales/en/common.json'
import en from '@/locales/en/linkgrabber.json'
import { loadEveryLocale } from '@/test/locales'

import CollectorCandidateRow from './CollectorCandidateRow.vue'

// The row reads which providers have an account; the answer does not matter here.
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn().mockResolvedValue({ data: [] }) },
  responseError: vi.fn(),
  errorMessage: vi.fn()
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en, common } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: { template: '<button v-bind="$attrs"><slot /></button>' },
  UCheckbox: passthrough,
  UBadge: passthrough,
  UIcon: passthrough,
  USelect: passthrough,
  UDropdownMenu: passthrough
}

beforeAll(loadEveryLocale)
beforeEach(() => setActivePinia(createPinia()))
afterEach(() => {
  appI18n.global.locale.value = 'en'
})

const detail = 'ERROR: [youtube] gjTzGhcQ21E: Video unavailable'

/** A playlist link whose yt-dlp run failed, as the list answers it. */
function failed(params: Record<string, string> | undefined): LinkCandidate {
  return {
    id: 'candidate-1',
    batch_id: 'batch-1',
    url: 'https://www.youtube.com/playlist?list=PLgone',
    state: 'error',
    file_name: 'playlist',
    created_at: '2026-10-11T10:00:00Z',
    priority: 'normal',
    position: 1,
    error: `yt-dlp failed: ${detail}`,
    error_code: 'media.ytdlp_failed',
    error_params: params
  } as LinkCandidate
}

function errorLine(value: LinkCandidate): string {
  render(CollectorCandidateRow, {
    props: { candidate: value, selected: false, busy: false },
    global: { plugins: [i18n], components }
  })
  return document.querySelector('p.text-error')?.textContent ?? ''
}

// RD-1250-03: the row stores the parameters of its message, so a coded failure reads in the
// interface's language with its cause, rather than in English or with an empty `{detail}`.
describe('CollectorCandidateRow and the parameters of its message (RD-1250-03)', () => {
  it.each([
    ['de', 'yt-dlp fehlgeschlagen: '],
    ['en', 'yt-dlp failed: '],
    ['es', 'yt-dlp falló: '],
    ['fr', 'Échec de yt-dlp : ']
  ])('translates a failure with its detail in %s', (locale, prefix) => {
    appI18n.global.locale.value = locale as never
    expect(errorLine(failed({ detail }))).toBe(`${prefix}${detail}`)
  })

  it('keeps the server text for a row stored without parameters', () => {
    appI18n.global.locale.value = 'de'
    expect(errorLine(failed(undefined))).toBe(`yt-dlp failed: ${detail}`)
  })
})
