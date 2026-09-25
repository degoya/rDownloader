import { describe, expect, it } from 'vitest'

import { SETTINGS_SECTIONS } from '@/settingsSections'

/**
 * The section list drives the routes, the sidebar and the rendered panel. When it was two lists
 * inside SettingsView they drifted, and `bandwidth` and `notifications` became unreachable by
 * link. This holds the shared list and what the view actually renders together.
 */
describe('settings sections', () => {
  const source = import.meta.glob('./SettingsView.vue', {
    eager: true,
    query: '?raw',
    import: 'default'
  })['./SettingsView.vue'] as string

  it('renders exactly the sections the shared list declares', () => {
    const rendered = [...source.matchAll(/v-if="activeSection === '(\w+)'"/g)].map(match => match[1])
    const declared = SETTINGS_SECTIONS.map(section => section.value)

    expect(rendered.length).toBe(declared.length)
    expect([...rendered].sort()).toEqual([...declared].sort())
  })

  it('places MFA and passkeys side by side on large screens', () => {
    const security = import.meta.glob('../components/settings/SettingsSecurityTab.vue', {
      eager: true,
      query: '?raw',
      import: 'default'
    })['../components/settings/SettingsSecurityTab.vue'] as string
    expect(security).toContain('class="grid items-start gap-6 lg:grid-cols-2"')
    expect(security).toMatch(/lg:grid-cols-2[\s\S]*<SettingsMfaCard \/>[\s\S]*<SettingsPasskeysCard \/>/)
  })
})

/**
 * RD-130-13: post-processing, BitTorrent and media (with galleries and streams) stood in
 * `md:grid-cols-2`, pairing settings that have nothing to do with each other, while every other
 * settings page had gone to one setting per row (`design.md`, RD-120-27). Read from the source,
 * because a card renders a branch only when its capability is on, and a grid in a branch the
 * test does not reach would pass a rendered check.
 */
describe('one setting per row on the tool pages', () => {
  const cards = import.meta.glob(
    [
      '../components/SettingsPostprocessCard.vue',
      '../components/SettingsTorrentCard.vue',
      '../components/SettingsMediaCard.vue',
      '../components/SettingsGalleryCard.vue',
      '../components/SettingsStreamCard.vue'
    ],
    { eager: true, query: '?raw', import: 'default' }
  ) as Record<string, string>

  /** `grid-cols-N` or `col-span-N` with N > 1, at any breakpoint prefix. */
  function sideBySide(source: string): string[] {
    return [...source.matchAll(/(?:[a-z0-9]+:)*(?:grid-cols|col-span)-(\d+)/g)]
      .filter(match => Number(match[1]) > 1)
      .map(match => match[0])
  }

  it('reads all five cards', () => {
    expect(Object.keys(cards)).toHaveLength(5)
  })

  it('puts no setting beside another', () => {
    for (const [path, source] of Object.entries(cards)) {
      const found = sideBySide(source)
      if (path.endsWith('SettingsPostprocessCard.vue')) {
        // The one row of alike values `design.md` keeps: the two archive ceilings, unhinted
        // figures read together, and nothing else in that grid.
        expect(found, path).toEqual(['sm:grid-cols-2'])
        const grid = source.slice(source.indexOf('sm:grid-cols-2'))
        const body = grid.slice(0, grid.indexOf('</div>'))
        expect(body.match(/<UFormField/g), path).toHaveLength(2)
        expect(body).toContain("t('settings.postprocess.max_files')")
        expect(body).toContain("t('settings.postprocess.max_bytes')")
      } else {
        expect(found, path).toEqual([])
      }
    }
  })
})

/**
 * The display preferences — unit ladder, pinned magnitude, tab title, subscription images —
 * live in module refs that every view formats through, not in this component's own object.
 * `save()` used to write the response back with a bare `Object.assign`, so a saved change
 * reached the rest of the interface only after a reload. This holds the two apart.
 */
describe('saving the display preferences', () => {
  const source = import.meta.glob('./SettingsView.vue', {
    eager: true,
    query: '?raw',
    import: 'default'
  })['./SettingsView.vue'] as string

  it('routes a saved settings document through applyLoadedSettings', () => {
    const save = source.slice(source.indexOf('async function save('))
    const body = save.slice(0, save.indexOf('\n}'))

    expect(body).toContain('applyLoadedSettings(response.data)')
    expect(body).not.toContain('Object.assign(settings, response.data)')
  })

  it('applies every module-level display preference in one place', () => {
    const apply = source.slice(source.indexOf('function applyLoadedSettings('))
    const body = apply.slice(0, apply.indexOf('\n}'))

    for (const setter of ['setByteDisplay', 'setByteUnit', 'setShowItemImages', 'setTitleStatus']) {
      expect(body).toContain(setter)
    }
  })
})
