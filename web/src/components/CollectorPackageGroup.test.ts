import { render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { CollectorPackage, LinkCandidate } from '@/api/types'
import common from '@/locales/en/common.json'
import en from '@/locales/en/linkgrabber.json'

import CollectorPackageGroup from './CollectorPackageGroup.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en, common } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: { template: '<button v-bind="$attrs" :disabled="$attrs.disabled"><slot /></button>' },
  UCheckbox: passthrough,
  UBadge: passthrough,
  UIcon: passthrough,
  USelect: passthrough
}

function candidate(state: LinkCandidate['state']): LinkCandidate {
  return {
    id: `candidate-${state}`,
    batch_id: 'batch-1',
    url: 'https://files.example.com/report.pdf',
    state,
    file_name: 'report.pdf',
    created_at: '2026-09-02T10:00:00Z',
    priority: 'normal',
    position: 1
  } as LinkCandidate
}

function renderGroup(candidates: LinkCandidate[]) {
  return render(CollectorPackageGroup, {
    props: {
      package: { id: 'package-1', name: 'Report', priority: 'normal', has_password: false } as CollectorPackage,
      candidates,
      categories: [],
      selectedIds: new Set<string>(),
      enqueuingIds: new Set<string>(),
      dragging: false,
      open: false
    },
    global: { plugins: [i18n], components }
  })
}

/** The pair of buttons `design.md` requires: the primary enqueue and its paused variant. */
function actionButtons(container: Element): { enqueue: HTMLButtonElement, paused: HTMLButtonElement } {
  const buttons = [...container.querySelectorAll('button')] as HTMLButtonElement[]
  const enqueue = buttons.find(button => button.getAttribute('label') === en.actions.enqueue)
  const paused = buttons.find(button => button.getAttribute('label') === en.actions.enqueue_paused)
  expect(enqueue).toBeTruthy()
  expect(paused).toBeTruthy()
  return { enqueue: enqueue as HTMLButtonElement, paused: paused as HTMLButtonElement }
}

describe('CollectorPackageGroup', () => {
  // `design.md`: an action with a start-mode variant offers the variant as a second button. The
  // package row offered "Enqueue" alone while the NZB row next to it in the same list offered the
  // pair, so the same list behaved in two ways (RD-107-09).
  it('offers the paused variant beside the enqueue button, with the documented icons', () => {
    const { container } = renderGroup([candidate('online')])
    const { enqueue, paused } = actionButtons(container)

    expect(enqueue.getAttribute('icon')).toBe('i-lucide-arrow-down-to-line')
    expect(paused.getAttribute('icon')).toBe('i-lucide-pause')
    expect(paused.getAttribute('color')).toBe('neutral')
    expect(paused.getAttribute('variant')).toBe('outline')
    expect(paused.getAttribute('title')).toBe(en.package.enqueue_paused_hint)
  })

  it('emits enqueuePaused with the package id', async () => {
    const { container, emitted } = renderGroup([candidate('online')])
    const { paused } = actionButtons(container)

    paused.click()
    await Promise.resolve()

    expect(emitted().enqueuePaused).toEqual([['package-1']])
    expect(emitted().enqueue).toBeUndefined()
  })

  // The binding text: "Both are enabled by exactly the same condition." A variant that is dead
  // while the primary works reads as a broken feature.
  it.each([
    ['an enqueueable link', [candidate('online')], false],
    ['a link that failed its check', [candidate('error')], false],
    ['no enqueueable link', [candidate('checking')], true],
    ['no link at all', [], true]
  ])('enables both buttons alike for %s', (_case, candidates, expectedDisabled) => {
    const { container } = renderGroup(candidates as LinkCandidate[])
    const { enqueue, paused } = actionButtons(container)

    expect(enqueue.hasAttribute('disabled')).toBe(expectedDisabled)
    expect(paused.hasAttribute('disabled')).toBe(enqueue.hasAttribute('disabled'))
  })
})
