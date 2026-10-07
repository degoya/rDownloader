/**
 * RD-1140-05: the global package-name rules — four switches, all off by default — and the preview
 * of the example name, which the service computes from the switches as they stand, saved or not.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const post = vi.fn()
vi.mock('@/api/client', () => ({ api: { POST: (...args: unknown[]) => post(...args) } }))

const { default: SettingsPackageNameRules } = await import('./SettingsPackageNameRules.vue')

const labels = settings.postprocess.package_names

function mount(model: Settings) {
  return mountComponent(SettingsPackageNameRules, { messages: { settings }, props: { modelValue: model } })
}

describe('SettingsPackageNameRules', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockResolvedValue({ data: { name: 'Big.Buck.Bunny.', folder: 'Big.Buck.Bunny', rules: {} } })
  })

  it('writes a switch into the settings document, the others staying off', async () => {
    const model = {} as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: labels.spaces_to_dots.label }))

    expect(model.package_name_rules).toEqual({ spaces_to_dots: true, collapse_separators: false, strip_bracket_tags: false, lowercase: false })
  })

  it('previews the example name under the switches as they stand', async () => {
    const model = { package_name_rules: { spaces_to_dots: true, strip_bracket_tags: true } } as Settings
    mount(model)

    await waitFor(() => expect(screen.getByTestId('package-names-preview').textContent).toContain('Big.Buck.Bunny'))
    const [path, request] = post.mock.calls[0] as [string, { body: { name: string, rules: Record<string, boolean>, regex: unknown } }]
    expect(path).toBe('/api/v1/postprocess/package-name-preview')
    expect(request.body.name).toBe('Big Buck Bunny [1080p]')
    // Every switch spelled out: an unsaved "off" must not fall back to a saved "on".
    expect(request.body.rules).toEqual({ spaces_to_dots: true, collapse_separators: false, strip_bracket_tags: true, lowercase: false })
    // The list as it stands, an empty one included: no saved pair slips into the preview.
    expect(request.body.regex).toEqual([])
  })

  it('shows why the service refuses a regex list instead of an example', async () => {
    post.mockResolvedValue({ error: { code: 'settings.package_name_regex_invalid', message: 'rule 1', params: { index: '1', detail: 'unclosed group' } } })
    mount({ package_name_regex: [{ pattern: '(', replacement: '' }] } as Settings)

    await waitFor(() => expect(screen.getByTestId('package-names-refusal')).toBeTruthy())
    expect(screen.queryByTestId('package-names-preview')).toBeNull()
  })
})
