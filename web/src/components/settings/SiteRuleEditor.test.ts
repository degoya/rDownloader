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
import { mountComponent, openablePopover } from '@/test/mount'

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
    },
    stubs: { UPopover: openablePopover }
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

  // "Service last seen alive" is typed or picked in the calendar beside it (RD-1140-09).
  it('takes the day the service was last seen alive from the calendar', async () => {
    const { model } = mount()
    const checked = screen.getByLabelText(siterules.editor.checked) as HTMLInputElement
    await fireEvent.update(checked, '2026-09-01')
    expect(model.value.checked).toBe('2026-09-01')
    await fireEvent.click(screen.getByRole('button', { name: common.date_field.open_calendar }))
    await fireEvent.click(screen.getByRole('button', { name: '2026-09-17' }))
    expect(model.value.checked).toBe('2026-09-17')
    expect(checked.value).toBe('2026-09-17')
  })

  it('asks for a trial run against the address the person names', async () => {
    const { emitted } = mount()
    await fireEvent.click(screen.getByText('Run the rule'))
    // No address typed, so the probe stands in for it rather than the button being dead.
    expect(emitted().test).toEqual([['https://example.org/release/1']])
  })

  it('never shows the probe, a real release, as the trial field\'s example (RD-1190-17)', async () => {
    const { model, emitted } = mount()
    model.value.probe = 'https://board.example/detail/9IMDqgvdVQQ6/A-Real-Release'
    const field = screen.getByLabelText('Address to try') as HTMLInputElement
    expect(field.placeholder).toBe('https://example.org/release/1')
    // An empty field still tries the probe, as the hint says.
    await fireEvent.click(screen.getByText('Run the rule'))
    expect(emitted().test).toEqual([['https://board.example/detail/9IMDqgvdVQQ6/A-Real-Release']])
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

describe('the editor for a page with several releases (RD-1170-02)', () => {
  it('draws the per-entry form behind its switch, with a step list of its own', async () => {
    const { model } = mount()
    expect(screen.getAllByLabelText('Kind')).toHaveLength(2)
    expect(screen.queryByLabelText(siterules.packages.from)).toBeNull()

    await fireEvent.click(screen.getByRole('switch', { name: siterules.packages.enabled }))
    expect(model.value.grouped).toBe(true)
    // The rule's two steps and the entry's one: the same rows, a second list.
    expect(screen.getAllByLabelText('Kind')).toHaveLength(3)
    await fireEvent.update(screen.getByLabelText(siterules.packages.from), 'releases')
    expect(model.value.groups.from).toBe('releases')
    expect(screen.getByText(siterules.packages.mirrors_none)).toBeTruthy()
    // With groups, mirrors are said per group, so the page-wide box is off limits.
    const pageWide = screen.getByLabelText(siterules.editor.mirrors) as HTMLInputElement
    expect(pageWide.disabled || pageWide.getAttribute('aria-disabled') === 'true' || pageWide.hasAttribute('data-disabled')).toBe(true)
  })

  it('shows one block per package the run found, each link with its mirror set', () => {
    mount({
      testResult: {
        address: 'https://example.org/release/1',
        package_name: 'Show',
        pages_fetched: 1,
        mirrors: false,
        kept: 3,
        refused: 0,
        error: null,
        links: [
          { url: 'https://one.example/a1', verdict: 'claimed', code: null },
          { url: 'https://two.example/b1', verdict: 'claimed', code: null },
          { url: 'https://one.example/c1', verdict: 'unconfirmed', code: null }
        ],
        groups: [
          {
            name: 'Show.S01.720p',
            links: [
              { url: 'https://one.example/a1', mirror: 1 },
              { url: 'https://two.example/b1', mirror: 1 }
            ]
          },
          { name: null, links: [{ url: 'https://one.example/c1', mirror: null }] }
        ]
      }
    })
    expect(screen.getByText('2 packages')).toBeTruthy()
    const first = screen.getByRole('region', { name: 'Show.S01.720p' })
    expect(within(first).getByText('2 links')).toBeTruthy()
    expect(within(first).getAllByText('Mirror 1')).toHaveLength(2)
    const a1 = within(first).getByText('https://one.example/a1').parentElement as HTMLElement
    expect(within(a1).getByText(siterules.test.verdicts.claimed)).toBeTruthy()
    const second = screen.getByRole('region', { name: siterules.test.group_unnamed })
    expect(within(second).getByText('1 link')).toBeTruthy()
    expect(within(second).queryByText(/Mirror/)).toBeNull()
  })
})

// The component does not fetch, so nothing here needs the API client.
vi.mock('@/api/client', () => ({ api: {}, responseError: () => '', resultMessage: () => '' }))

describe('the editor as a form (RD-150-11)', () => {
  function complete(): RuleDraft {
    const value = draft()
    value.group = 'board'
    return value
  }

  it('saves on Enter and ends with the create action, the active switch above it', async () => {
    const { emitted } = mountComponent(SiteRuleEditor, {
      messages: { common, server, siterules },
      props: { modelValue: complete(), editingId: null, pending: false, testResult: null, groups: [] }
    })
    const actions = Array.from(document.querySelectorAll('[data-form-actions] button')).map(button => button.textContent)
    expect(actions).toEqual([siterules.editor.create])
    expect(screen.getByRole('switch', { name: siterules.editor.enabled })).toBeTruthy()

    await fireEvent.submit(screen.getByLabelText(siterules.editor.name).closest('form') as HTMLFormElement)
    expect(emitted().save).toHaveLength(1)
  })

  it('offers save and the icon-only cross while a rule is edited', async () => {
    const { emitted } = mountComponent(SiteRuleEditor, {
      messages: { common, server, siterules },
      props: { modelValue: complete(), editingId: 'my-board', pending: false, testResult: null, groups: [] }
    })
    const [save, cancel] = Array.from(document.querySelectorAll('[data-form-actions] button'))
    expect(save?.textContent).toBe(common.actions.save)
    expect(cancel?.getAttribute('aria-label')).toBe(common.actions.cancel_edit)
    await fireEvent.click(cancel as HTMLElement)
    expect(emitted().cancel).toHaveLength(1)
  })

  it('runs the test on Enter in the test address instead of saving', async () => {
    const { emitted } = mountComponent(SiteRuleEditor, {
      messages: { common, server, siterules },
      props: { modelValue: complete(), editingId: null, pending: false, testResult: null, groups: [] }
    })
    const address = screen.getByLabelText(siterules.test.address)
    await fireEvent.update(address, 'https://example.org/release/2')
    await fireEvent.keyDown(address, { key: 'Enter' })
    expect(emitted().test).toEqual([['https://example.org/release/2']])
    expect(emitted().save).toBeUndefined()
  })
})
