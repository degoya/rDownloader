/**
 * The About page (RD-130-12), checked on what the job's acceptance criteria ask of the screen:
 * the build's figures come from the service and say "unknown" rather than nothing, an address
 * that is not published yet is text with a mark and never a link, and the dependency list is
 * behind a control that says how many rows it opens onto.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import en from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsAboutTab from './SettingsAboutTab.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn() },
  responseError: () => 'The service did not answer'
}))

const REPOSITORY = 'https://github.com/degoya/rDownloader'

function aboutAnswer(overrides: Record<string, unknown> = {}) {
  return {
    version: '1.3.0',
    commit: '1a2b3c4d',
    built: '2026-09-25T12:00:00Z',
    plugin_contracts: ['rdownloader:plugin@0.8.0'],
    platform: 'linux x86_64',
    license: 'GPL-3.0-or-later',
    authors: ['Alexander Herling'],
    links: [
      { kind: 'source', url: REPOSITORY, published: false },
      { kind: 'website', url: 'https://rdownloader.net', published: true },
      { kind: 'handbook', url: null, published: false },
      { kind: 'security', url: `${REPOSITORY}/security/advisories/new`, published: true },
      { kind: 'changelog', url: `${REPOSITORY}/blob/main/CHANGELOG.md`, published: true }
    ],
    bundled_tools: [
      { name: 'Streamlink', license: 'BSD-2-Clause', file: 'vendor/licenses/Streamlink-LICENSE.txt', homepage: 'https://streamlink.github.io' }
    ],
    ...overrides
  }
}

const LICENSES = {
  rust: [
    { name: 'axum', version: '0.8.4', license: 'MIT' },
    { name: 'ring', version: '0.17.14', license: 'Apache-2.0 AND ISC' },
    { name: 'serde', version: '1.0.219', license: 'MIT OR Apache-2.0' },
    { name: 'tokio', version: '1.47.0', license: 'MIT' }
  ],
  rust_not_shipped: ['tempfile@3.20.0'],
  npm: [{ name: 'vue', version: '3.5.42', license: 'MIT' }]
}

function answer(about: unknown) {
  vi.mocked(api.GET).mockImplementation((async (path: string) => ({
    data: path.endsWith('/licenses') ? LICENSES : about
  })) as never)
}

function mount() {
  return mountComponent(SettingsAboutTab, { messages: { settings: en } })
}

describe('SettingsAboutTab', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
  })

  it('shows the build the service reports, and says unknown for what it does not know', async () => {
    answer(aboutAnswer({ commit: null, built: null }))
    const { container } = mount()

    await waitFor(() => expect(container.querySelector('[data-fact="version"]')?.textContent).toBe('1.3.0'))
    expect(container.querySelector('[data-fact="commit"]')?.textContent).toBe('unknown')
    expect(container.querySelector('[data-fact="built"]')?.textContent).toBe('unknown')
    expect(container.querySelector('[data-fact="plugin_contract"]')?.textContent).toBe('rdownloader:plugin@0.8.0')
    expect(container.querySelector('[data-fact="platform"]')?.textContent).toBe('linux x86_64')
    expect(screen.getByText('Alexander Herling')).toBeTruthy()
  })

  it('writes an unpublished address out as text with a mark, and links only a published one', async () => {
    answer(aboutAnswer())
    const { container } = mount()

    await waitFor(() => expect(container.querySelectorAll('[data-link]')).toHaveLength(5))
    const source = container.querySelector('[data-link="source"]')
    expect(source?.textContent).toContain('Source code')
    expect(source?.textContent).toContain('not yet published')
    expect(source?.querySelector('a')).toBeNull()
    expect(container.querySelector('[data-link="changelog"] a')?.getAttribute('href')).toBe(`${REPOSITORY}/blob/main/CHANGELOG.md`)
    // An address nobody has decided yet is the mark alone, never a guess (the service sends
    // none today; the fixture keeps the case the schema allows).
    const handbook = container.querySelector('[data-link="handbook"]')
    expect(handbook?.textContent).toContain('Handbook')
    expect(handbook?.textContent).toContain('not yet published')
    expect(handbook?.textContent).not.toContain('https://')

    const website = container.querySelector('[data-link="website"] a')
    expect(website?.getAttribute('href')).toBe('https://rdownloader.net')
    expect(website?.getAttribute('rel')).toContain('noopener')
    expect(container.querySelector('[data-link="website"] [data-unpublished]')).toBeNull()
    // Every label resolves: none of them is the raw key.
    expect(container.querySelector('[data-testid="about-links"]')?.textContent).not.toContain('settings.about')
  })

  it('names the bundled tools with the file their license text is in', async () => {
    answer(aboutAnswer())
    const { container } = mount()

    await waitFor(() => expect(container.querySelector('[data-testid="about-tools"]')).toBeTruthy())
    const row = container.querySelector('[data-testid="about-tools"] tbody tr')
    expect(row?.textContent).toContain('Streamlink')
    expect(row?.textContent).toContain('BSD-2-Clause')
    expect(row?.textContent).toContain('vendor/licenses/Streamlink-LICENSE.txt')
  })

  it('summarises each dependency list and opens it onto the rows it counted', async () => {
    answer(aboutAnswer())
    const { container } = mount()

    await waitFor(() => expect(container.querySelector('[data-toggle="rust"]')).toBeTruthy())
    const rust = container.querySelector('[data-ecosystem="rust"]')
    expect(rust?.textContent).toContain('Rust crates')
    expect(rust?.textContent).toContain('MIT · 2')
    expect(container.querySelector('[data-ecosystem="npm"]')?.textContent).toContain('npm packages of the interface')
    // Closed, the rows are not drawn and the button says how many it opens onto.
    expect(container.querySelector('[data-list="rust"]')).toBeNull()
    expect(container.querySelector('[data-testid="about-filter"]')).toBeNull()
    const toggle = container.querySelector('[data-toggle="rust"]') as HTMLElement
    expect(toggle.textContent).toContain('Show all 4')

    await fireEvent.click(toggle)
    expect(container.querySelectorAll('[data-list="rust"] tbody tr')).toHaveLength(4)
    // The crates that ship nowhere are never listed as if they did.
    expect(container.textContent).not.toContain('tempfile')

    const filter = container.querySelector('[data-testid="about-filter"]') as HTMLInputElement
    await fireEvent.update(filter, 'apache')
    expect(container.querySelectorAll('[data-list="rust"] tbody tr')).toHaveLength(2)
    await fireEvent.update(filter, 'nothing-like-this')
    expect(container.querySelector('[data-ecosystem="rust"]')?.textContent).toContain('No package matches the filter.')
  })

  it('says so when the service does not answer, instead of an empty page', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: undefined, error: { code: 'internal' } } as never)
    const { container } = mount()

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('The service did not answer'))
    expect(container.querySelector('[data-testid="about-build"]')).toBeNull()
  })
})
