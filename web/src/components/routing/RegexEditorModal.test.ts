/**
 * RD-1140-05: the regex editor's replacement mode, for the package-name regex rules. Given a
 * `replacement`, it shows a "Replace with" field, tries the pair on package names with the
 * service's engine and hands back pattern and replacement; without one it is the find-only
 * editor the routing rules use, unchanged.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const post = vi.fn()
vi.mock('@/api/client', () => ({ api: { POST: (...args: unknown[]) => post(...args) } }))

const { default: RegexEditorModal } = await import('./RegexEditorModal.vue')

/** The dialog with its regions rendered, so the body and the footer are reachable. */
const UModal = { props: ['title'], template: '<div><h2>{{ title }}</h2><slot name="body" /><slot name="footer" /></div>' }

const editor = routing.rule.regex_editor

describe('RegexEditorModal replacement mode', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockImplementation(async (_path: string, { body }: { body: { samples: string[] } }) => ({
      data: {
        valid: true,
        error: null,
        results: body.samples.map(sample => ({ matched: true, start: 0, end: 1, replaced: sample.replaceAll('_', '.') }))
      }
    }))
  })

  it('tries the pair on package names and shows what each becomes', async () => {
    mountComponent(RegexEditorModal, { messages: { routing }, props: { pattern: '_', replacement: '.' }, stubs: { UModal } })

    expect(screen.getByText(editor.replacement_title)).toBeTruthy()
    await waitFor(() => expect(post).toHaveBeenCalled())
    const request = post.mock.calls[0]?.[1] as { body: { pattern: string, samples: string[], replacement: string } }
    expect(request.body).toMatchObject({ pattern: '_', replacement: '.' })
    expect(request.body.samples).toContain('Sintel_Directors_Cut_Update_v1.0.2_EXAMPLE')
    await waitFor(() => expect(screen.getByTestId('regex-replaced').textContent).toContain('Sintel.Directors.Cut.Update.v1.0.2.EXAMPLE'))
  })

  it('hands back the pattern and the replacement', async () => {
    const { emitted } = mountComponent(RegexEditorModal, { messages: { routing, common }, props: { pattern: '_', replacement: '' }, stubs: { UModal } })

    await fireEvent.update(screen.getByTestId('regex-replacement'), ' ')
    await waitFor(() => expect(post.mock.calls.some(([, request]) => (request as { body: { replacement?: string } }).body.replacement === ' ')).toBe(true))
    await fireEvent.click(screen.getByRole('button', { name: common.actions.apply }))

    expect(emitted().close?.at(-1)).toEqual([{ pattern: '_', replacement: ' ' }])
  })
})

describe('RegexEditorModal without a replacement', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockResolvedValue({ data: { valid: true, error: null, results: [] } })
  })

  it('stays the find-only editor', async () => {
    const { emitted } = mountComponent(RegexEditorModal, { messages: { routing, common }, props: { pattern: '1080p' }, stubs: { UModal } })

    await waitFor(() => expect(post).toHaveBeenCalled())
    expect(post.mock.calls[0]?.[1]).toEqual({ body: { pattern: '1080p', samples: ['Movie.2024.1080p.x265.mkv', 'Show.S01E01.720p.mp4'] } })
    expect(screen.queryByTestId('regex-replacement')).toBeNull()
    expect(screen.getByText(editor.title)).toBeTruthy()
    await waitFor(() => expect((screen.getByRole('button', { name: common.actions.apply }) as HTMLButtonElement).disabled).toBe(false))
    await fireEvent.click(screen.getByRole('button', { name: common.actions.apply }))
    expect(emitted().close?.at(-1)).toEqual([{ pattern: '1080p' }])
  })
})

describe('RegexEditorModal diagram (RD-1140-06)', () => {
  beforeEach(() => post.mockReset())

  it('draws the pattern under it from the tester\'s structure', async () => {
    post.mockResolvedValue({ data: { valid: true, error: null, results: [], structure: { kind: 'sequence', children: [{ kind: 'start' }, { kind: 'repetition', min: 1, children: [{ kind: 'digit' }] }, { kind: 'end' }] } } })
    mountComponent(RegexEditorModal, { messages: { routing }, props: { pattern: '^\\d+$' }, stubs: { UModal } })

    await waitFor(() => expect(screen.getByRole('group', { name: editor.diagram.label })).toBeTruthy())
    expect(screen.getAllByRole('listitem').map(item => item.textContent)).toEqual(['Starts with', 'then Any digit (one or more times)', 'then Ends here'])
  })

  it('shows the error instead of a diagram for an invalid pattern', async () => {
    post.mockResolvedValue({ data: { valid: false, error: 'regex parse error', results: [] } })
    mountComponent(RegexEditorModal, { messages: { routing }, props: { pattern: '(' }, stubs: { UModal } })

    await waitFor(() => expect(screen.getByText('regex parse error')).toBeTruthy())
    expect(screen.getByText(editor.invalid_pattern)).toBeTruthy()
    expect(screen.queryByRole('group', { name: editor.diagram.label })).toBeNull()
  })
})
