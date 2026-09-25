/**
 * The editor's two halves (RD-110-08): the steps are named fields rather than a JSON box, and
 * a rule can be tried against a real address before it is saved.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import siterules from '@/locales/en/siterules.json'
import { emptyDraft, emptyStep, type RuleDraft } from '@/composables/useSiteRules'
import { mountComponent } from '@/test/mount'

import SiteRuleEditor from './SiteRuleEditor.vue'

function draft(): RuleDraft {
  const value = emptyDraft()
  value.id = 'my-board'
  value.name = 'My board'
  value.hosts = 'example.org'
  value.probe = 'https://example.org/release/1'
  value.steps = [emptyStep('fetch'), { ...emptyStep('regex'), pattern: 'href="([^"]+)"', into: 'links', all: true }]
  return value
}

function mount(options: { testResult?: unknown, groups?: string[] } = {}) {
  const model = ref(draft())
  const rendered = mountComponent(SiteRuleEditor, {
    messages: { common, server, siterules },
    props: {
      modelValue: model.value,
      editingId: null,
      pending: false,
      testResult: options.testResult ?? null,
      groups: options.groups ?? []
    }
  })
  return { ...rendered, model }
}

/** The items the group field offers, in the order it offers them. */
function groupMenu(): HTMLElement {
  const field = screen.getByLabelText('Group')
  return field.parentElement?.querySelector('[data-menu-items]') as HTMLElement
}

describe('the site-rule editor', () => {
  it('draws every step as its own row with the fields that kind needs', async () => {
    mount()
    // Two steps, each with a kind select, and every step writes into a variable.
    const kinds = screen.getAllByLabelText('Kind')
    expect(kinds).toHaveLength(2)
    expect((kinds[0] as HTMLSelectElement).value).toBe('fetch')
    expect((kinds[1] as HTMLSelectElement).value).toBe('regex')
    expect(screen.getAllByPlaceholderText('links')).toHaveLength(2)
    // A field only one kind has is only drawn for that kind — this is the point of the
    // editor: named fields per step, not one JSON box for all seven.
    expect(screen.getByText('Take every match')).toBeTruthy()
    expect(screen.queryByPlaceholderText('recaptcha-v2')).toBeNull()
    await fireEvent.update(kinds[0] as HTMLSelectElement, 'captcha')
    expect(screen.getByPlaceholderText('recaptcha-v2')).toBeTruthy()
    expect(screen.queryByText('Take every match')).toBeTruthy()

    // The order is meaning, so the first step cannot move up and the last cannot move down.
    expect((screen.getAllByLabelText('Move step up')[0] as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getAllByLabelText('Move step down')[1] as HTMLButtonElement).disabled).toBe(true)
  })

  it('puts one field per row, as a form beside its list must (RD-120-21)', () => {
    const { container } = mount()
    // This form is the left column of a `FormListLayout`, and `design.md` gives that column one
    // field per row. Three grids inside it paired identifier with name, group with revision and
    // probe with a date — labels and hints of different lengths, so the fields sat at different
    // heights and the columns had nothing to do with each other. Checked at the rendered
    // component rather than by eye, because that is what let it pass review the first time.
    expect(container.querySelectorAll('[class*="grid-cols-2"]')).toHaveLength(0)
    expect(container.querySelectorAll('[class*="col-span-2"]')).toHaveLength(0)
  })

  it('offers the groups the installation already has instead of asking for them from memory', async () => {
    const { model } = mount({ groups: ['board', 'forum'] })
    // The draft's own group is among them, so a rule opened for editing shows where it sits.
    expect(within(groupMenu()).getAllByRole('button').map(item => item.textContent))
      .toEqual(['board', 'forum'])
    await fireEvent.click(within(groupMenu()).getByText('forum'))
    expect(model.value.group).toBe('forum')
  })

  it('still lets a group be named that no rule carries yet', async () => {
    const { model } = mount({ groups: ['board', 'forum'] })
    // Nothing to create while the typed text names something that exists.
    expect(groupMenu().querySelector('[data-create-item]')).toBeNull()
    await fireEvent.update(screen.getByLabelText('Group'), 'gallery')
    const create = groupMenu().querySelector('[data-create-item]') as HTMLElement
    expect(create).toBeTruthy()
    await fireEvent.click(create)
    expect(model.value.group).toBe('gallery')
    // And it joins the offer, so the next rule of that group is chosen rather than retyped —
    // the typo that quietly opens a second group of one is what this field is for.
    expect(within(groupMenu()).getAllByRole('button').map(item => item.textContent))
      .toContain('gallery')
  })

  it('asks for a trial run against the address the person names', async () => {
    const { emitted } = mount()
    await fireEvent.click(screen.getByText('Run the rule'))
    // No address typed, so the probe stands in for it rather than the button being dead.
    expect(emitted().test).toEqual([['https://example.org/release/1']])
  })

  it('shows what the run found: the links, the package name and what was dropped', () => {
    mount({
      testResult: {
        address: 'https://example.org/release/1',
        package_name: 'Some.Release.1080p',
        pages_fetched: 2,
        mirrors: true,
        kept: 1,
        refused: 1,
        error: null,
        links: [
          { url: 'https://hoster.example/file', verdict: 'claimed', code: null },
          { url: 'https://example.org/forum', verdict: 'not-a-file', code: 'collector.crawl_not_a_file' }
        ]
      }
    })
    expect(screen.getByText('Package name: Some.Release.1080p')).toBeTruthy()
    expect(screen.getByText('1 kept')).toBeTruthy()
    expect(screen.getByText('1 dropped')).toBeTruthy()
    expect(screen.getByText('https://hoster.example/file')).toBeTruthy()
    const dropped = screen.getByText('https://example.org/forum').parentElement as HTMLElement
    expect(within(dropped).getByText('Answers with a page, not a file')).toBeTruthy()
  })

  it('states the refusal in the reader’s language when the run produced nothing', () => {
    mount({
      testResult: {
        address: 'https://example.org/release/1',
        package_name: null,
        pages_fetched: 0,
        mirrors: false,
        kept: 0,
        refused: 0,
        error: 'site_rules.structure',
        links: []
      }
    })
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toBe(server.codes['site_rules.structure'])
  })
})

// The component does not fetch, so nothing here needs the API client.
vi.mock('@/api/client', () => ({ api: {}, responseError: () => '', resultMessage: () => '' }))
