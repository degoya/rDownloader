/**
 * The sidebar's collapse switch and the `0` shortcut behind it (RD-109-31), what the
 * collapsed rail shows in its header and footer (RD-110-32), and the entries the navigation
 * lists — remote jobs beside the subscriptions, the settings under their six rubrics (RD-110-29).
 *
 * `UDashboardSidebarCollapse` cannot be imported here: it resolves `#imports` and
 * `#build/ui/...`, aliases that only exist once `@nuxt/ui/vite` runs, and the Vitest config
 * deliberately does not load that plugin (see `useAppShortcuts.test.ts` for the same
 * constraint). What the two stubs below reproduce is therefore not a guess but the real
 * components' wiring, taken from their sources and using the very same context helpers:
 * `UDashboardSidebar` provides the dashboard context and mirrors `collapsed` as a model,
 * `UDashboardSidebarCollapse` reads `sidebarCollapsed` from that context and calls
 * `collapseSidebar(!sidebarCollapsed)` on click. Everything between the click and the state —
 * our `v-model:collapsed` binding, our accessible name, our shared ref — is this app's code and
 * is what these tests cover. The library's own pixel behaviour (the 64px rail, the icon swap)
 * is a browser matter and is not claimed here.
 */
import { provideDashboardContext, useDashboard } from '@nuxt/ui/utils/dashboard'
import { fireEvent, render, screen } from '@testing-library/vue'
import axe from 'axe-core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { computed, defineComponent, nextTick, ref, toRef } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn()
}))
// Both pull in `@nuxt/ui/composables`, which needs the `#imports` alias.
vi.mock('@/composables/useAppShortcuts', () => ({ useAppShortcuts: () => {} }))
vi.mock('./CaptchaDialog.vue', () => ({ default: { template: '<div />' } }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))
vi.mock('@/composables/useNzbDropZone', () => ({ useFileImportDropZone: () => ({ dropActive: ref(false) }) }))
vi.mock('@/stores/transfers', () => ({ useTransfersStore: () => ({ packages: [], activePackages: 0 }) }))
vi.mock('@/stores/collector', () => ({ useCollectorStore: () => ({ packages: [] }) }))
vi.mock('@/stores/nzbImports', () => ({ useNzbImportsStore: () => ({ imports: [] }) }))
vi.mock('@/stores/streams', () => ({ useStreamsStore: () => ({ channels: [] }) }))
vi.mock('@/stores/session', () => ({
  useSessionStore: () => ({ consumeTourRequest: () => false, loginDisabled: true, pending: false, logout: vi.fn() })
}))

import ControlRoomLayout from './ControlRoomLayout.vue'
import { i18n, SUPPORTED_LOCALES } from '@/i18n'
import { sidebarCollapsed } from '@/composables/sidebarCollapse'
import { SETTINGS_SECTION_GROUPS } from '@/settingsSections'

/** Mirrors `@nuxt/ui`'s `DashboardSidebar`: provides the context, exposes `collapsed` as a model. */
const UDashboardSidebar = defineComponent({
  props: { collapsed: { type: Boolean, default: false } },
  emits: { 'update:collapsed': (value: boolean) => typeof value === 'boolean' },
  setup(props, { emit }) {
    provideDashboardContext({
      sidebarOpen: ref(false),
      sidebarCollapsed: toRef(props, 'collapsed'),
      collapseSidebar: (value: boolean) => emit('update:collapsed', value)
    })
    return { isCollapsed: computed(() => props.collapsed) }
  },
  template: `<div data-testid="sidebar" :data-collapsed="String(isCollapsed)">
    <slot name="header" :collapsed="isCollapsed" />
    <slot :collapsed="isCollapsed" />
    <slot name="footer" :collapsed="isCollapsed" />
  </div>`
})

/** Mirrors `@nuxt/ui`'s `DashboardSidebarCollapse`: one button, one call into the context. */
const UDashboardSidebarCollapse = defineComponent({
  setup() {
    const { sidebarCollapsed: contextCollapsed, collapseSidebar } = useDashboard({
      sidebarCollapsed: ref(false),
      collapseSidebar: () => {}
    })
    return { contextCollapsed, collapseSidebar }
  },
  template: '<button type="button" v-bind="$attrs" @click="collapseSidebar?.(!contextCollapsed)" />'
})

const passthrough = { template: '<div><slot /></div>' }

/**
 * Mirrors the shape `UNavigationMenu` reads: every item of every list as a link with its
 * label, and an item's children under it. Enough to see what is listed and in which order;
 * the popover, the tooltips and the active styling are the library's and are not claimed.
 */
const UNavigationMenu = defineComponent({
  props: { items: { type: Array, required: true }, modelValue: { type: [Array, String], default: undefined } },
  emits: ['update:modelValue'],
  template: `<div data-testid="menu">
    <template v-for="item in items.flat()" :key="item.to ?? item.value">
      <a
        v-if="item.to"
        :href="item.to"
        :data-icon="item.icon"
        :data-active="item.active ? 'true' : 'false'"
        :data-open="item.children ? String((modelValue ?? []).includes(item.value)) : undefined"
      >{{ item.label }}</a>
      <button v-if="item.children" type="button" data-testid="toggle-group" @click="$emit('update:modelValue', (modelValue ?? []).includes(item.value) ? (modelValue ?? []).filter(v => v !== item.value) : [...(modelValue ?? []), item.value])" />
      <div v-if="item.children" data-testid="settings-children">
        <template v-for="child in item.children" :key="child.to ?? child.value">
          <span v-if="child.type === 'label'" data-group-label>{{ child.label }}</span>
          <a v-else :href="child.to">{{ child.label }}</a>
        </template>
      </div>
    </template>
  </div>`
})

function mountLayout() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', redirect: '/downloads' },
      { path: '/downloads', component: { template: '<p>downloads</p>' } },
      { path: '/linkgrabber', component: { template: '<p>linkgrabber</p>' } },
      { path: '/settings', component: { template: '<p>overview</p>' } },
      { path: '/settings/:section', component: { template: '<p>settings page</p>' } }
    ]
  })
  return {
    router,
    ...render(ControlRoomLayout, {
      global: {
        plugins: [router, i18n],
        stubs: {
          LiveAnnouncer: true,
          NzbDropOverlay: true,
          TransferRail: true,
          UButton: { template: '<button type="button" v-bind="$attrs" />' },
          UDashboardGroup: passthrough,
          UDashboardSidebar,
          UDashboardSidebarCollapse,
          UDropdownMenu: passthrough,
          UIcon: { template: '<span aria-hidden="true" />' },
          ULink: passthrough,
          UNavigationMenu,
          USelect: { props: ['modelValue', 'items', 'size', 'icon'], template: '<select v-bind="$attrs" />' },
          UTooltip: passthrough
        }
      }
    })
  }
}

/** The one button in the sidebar header; identified by its accessible name, in any language. */
function collapseButton(): HTMLElement {
  const label = i18n.global.t(sidebarCollapsed.value ? 'nav.expand_sidebar' : 'nav.collapse_sidebar')
  return screen.getByRole('button', { name: label })
}

/**
 * axe over the rendered shell. `region` is off for the same reason as in `accessibility.test.ts`:
 * the landmarks are this layout's own, and the stubs around it are not a page.
 */
async function violations(container: Element): Promise<string> {
  const results = await axe.run(container, { rules: { region: { enabled: false } } })
  return results.violations
    .map(violation => `${violation.id}: ${violation.help} (${violation.nodes.length} node(s))`)
    .join('\n')
}

beforeEach(() => {
  sidebarCollapsed.value = false
  i18n.global.locale.value = 'en'
})

describe('the sidebar collapse switch', () => {
  it('collapses the sidebar and expands it again', async () => {
    const { router } = mountLayout()
    await router.isReady()

    expect(screen.getByTestId('sidebar').dataset.collapsed).toBe('false')

    await fireEvent.click(collapseButton())
    expect(sidebarCollapsed.value).toBe(true)
    expect(screen.getByTestId('sidebar').dataset.collapsed).toBe('true')

    await fireEvent.click(collapseButton())
    expect(sidebarCollapsed.value).toBe(false)
    expect(screen.getByTestId('sidebar').dataset.collapsed).toBe('false')
  })

  it('wears the logo as its face on the collapsed rail, and stays the way back out', async () => {
    const { router } = mountLayout()
    await router.isReady()

    expect(screen.getByAltText('rDownloader')).toBeTruthy()
    expect(collapseButton().style.backgroundImage).toBe('')
    await fireEvent.click(collapseButton())

    // 64px of rail, 32px of it between the header's paddings: one control, so the logo is
    // the switch. The wordmark's own image and text are gone; the mark is on the button.
    expect(screen.queryByAltText('rDownloader')).toBeNull()
    expect(screen.queryByText('rDownloader')).toBeNull()
    const logo = collapseButton()
    expect(logo.style.backgroundImage).toContain('favicon.svg')
    expect(logo.getAttribute('title')).toBe(i18n.global.t('nav.expand_sidebar'))

    // Still a button, so a keyboard reaches it, and it expands the sidebar again.
    expect(logo.tagName).toBe('BUTTON')
    expect(logo.tabIndex).toBe(0)
    await fireEvent.click(logo)
    expect(sidebarCollapsed.value).toBe(false)
    expect(screen.getByAltText('rDownloader')).toBeTruthy()
    expect(collapseButton().style.backgroundImage).toBe('')
  })

  it('keeps the keyboard focus through the switch, in both directions', async () => {
    const { router } = mountLayout()
    await router.isReady()

    // The same element in both states — a `v-if` pair would drop the focus onto the page.
    const button = collapseButton()
    button.focus()
    expect(document.activeElement).toBe(button)
    await fireEvent.click(button)
    expect(sidebarCollapsed.value).toBe(true)
    expect(document.activeElement).toBe(collapseButton())
    await fireEvent.click(collapseButton())
    expect(sidebarCollapsed.value).toBe(false)
    expect(document.activeElement).toBe(collapseButton())
  })

  it('names itself for the state it would produce, in all four languages', async () => {
    for (const locale of SUPPORTED_LOCALES) {
      sidebarCollapsed.value = false
      i18n.global.locale.value = locale
      const { router, unmount } = mountLayout()
      await router.isReady()

      const expand = i18n.global.t('nav.expand_sidebar')
      const collapse = i18n.global.t('nav.collapse_sidebar')
      expect(collapse).not.toBe('nav.collapse_sidebar')
      expect(expand).not.toBe(collapse)

      const button = screen.getByRole('button', { name: collapse })
      expect(button.getAttribute('title')).toBe(collapse)

      await fireEvent.click(button)
      expect(screen.getByRole('button', { name: expand }).getAttribute('title')).toBe(expand)
      unmount()
    }
  })
})

describe('the collapsed rail', () => {
  it('shows the gear for language and theme in the footer, and the two selects when expanded', async () => {
    const { router } = mountLayout()
    await router.isReady()

    const name = i18n.global.t('common.preferences.language_and_theme')
    expect(name).not.toBe('common.preferences.language_and_theme')
    expect(screen.queryByRole('button', { name })).toBeNull()
    expect(screen.getByRole('combobox', { name: i18n.global.t('common.preferences.language') })).toBeTruthy()
    expect(screen.getByRole('combobox', { name: i18n.global.t('common.preferences.theme') })).toBeTruthy()

    await fireEvent.click(collapseButton())
    const gear = screen.getByRole('button', { name })
    expect(gear.getAttribute('icon')).toBe('i-lucide-settings-2')
    expect(gear.getAttribute('title')).toBe(name)
    expect(screen.queryByRole('combobox')).toBeNull()
  })

  // jsdom lays nothing out, so the widths are measured in Chromium (RD-120-61 job file); what
  // is pinned here is why they came out wrong. The footer slot is a flex row: a block in it
  // without `w-full` shrinks to its content — 165 px of a 240 px sidebar, and on the 64 px rail
  // a 56 px connection row that pushed the separator 9 px past the edge.
  it('gives the footer the full width of the sidebar, open and on the rail', async () => {
    const { router } = mountLayout()
    await router.isReady()

    const footer = screen.getByTestId('sidebar-footer')
    expect(footer.classList).toContain('w-full')
    expect(footer.classList).toContain('border-t')
    // The selects carry no inset of their own, so they run edge to edge with the separator.
    const selects = screen.getByRole('combobox', { name: i18n.global.t('common.preferences.language') }).parentElement
    expect([...(selects?.classList ?? [])].filter(name => /^p[xlr]-/.test(name))).toEqual([])
    expect(screen.getByTestId('sidebar-connection').classList).not.toContain('flex-col')

    await fireEvent.click(collapseButton())
    expect(screen.getByTestId('sidebar-footer').classList).toContain('w-full')
    // Side by side, dot and sign-out need 56 px of the 32 px between the rail's paddings.
    const connection = screen.getByTestId('sidebar-connection')
    expect(connection.classList).toContain('flex-col')
    expect([...connection.classList].filter(name => /^p[xlr]-/.test(name))).toEqual([])
  })

  it('passes axe expanded and collapsed', async () => {
    const { router, container } = mountLayout()
    await router.isReady()
    expect(await violations(container)).toBe('')

    await fireEvent.click(collapseButton())
    expect(await violations(container)).toBe('')
  })
})

describe('the collapsed state and a page change', () => {
  it('stays collapsed across a route change', async () => {
    const { router } = mountLayout()
    await router.isReady()

    await fireEvent.click(collapseButton())
    await router.push('/linkgrabber')
    await Promise.resolve()

    expect(router.currentRoute.value.path).toBe('/linkgrabber')
    expect(sidebarCollapsed.value).toBe(true)
    expect(screen.getByTestId('sidebar').dataset.collapsed).toBe('true')
  })

  it('is session state, not one mount of the layout', async () => {
    const first = mountLayout()
    await first.router.isReady()
    await fireEvent.click(collapseButton())
    first.unmount()

    const second = mountLayout()
    await second.router.isReady()
    expect(screen.getByTestId('sidebar').dataset.collapsed).toBe('true')
  })
})

describe('the navigation entries', () => {
  it('lists remote jobs directly after the subscriptions, with a cloud icon', async () => {
    const { router } = mountLayout()
    await router.isReady()

    const links = [...screen.getByTestId('menu').querySelectorAll(':scope > a')]
    const hrefs = links.map(link => link.getAttribute('href'))
    expect(hrefs.indexOf('/remote-jobs')).toBe(hrefs.indexOf('/subscriptions') + 1)
    const entry = links.find(link => link.getAttribute('href') === '/remote-jobs')
    expect(entry?.textContent?.trim()).toBe(i18n.global.t('nav.remote_jobs'))
    expect(entry?.getAttribute('data-icon')).toBe('i-lucide-cloud-cog')
  })

  it('lists the settings pages under the six rubrics of the shared table, in its order', async () => {
    const { router } = mountLayout()
    await router.isReady()

    const children = screen.getByTestId('settings-children')
    const labels = [...children.querySelectorAll('[data-group-label]')].map(label => label.textContent?.trim())
    expect(labels).toEqual(SETTINGS_SECTION_GROUPS.map(group => i18n.global.t(group.labelKey)))
    const hrefs = [...children.querySelectorAll('a')].map(link => link.getAttribute('href'))
    expect(hrefs).toEqual(SETTINGS_SECTION_GROUPS.flatMap(group => group.sections.map(section => `/settings/${section.value}`)))
  })
})

/**
 * The settings group on a settings page (RD-120-53). The screenshot run showed it closed and
 * unhighlighted on all twenty-four pages: `defaultOpen` is read once, when the shell mounts,
 * and the pages are routes of their own rather than children of `/settings`, so the router
 * never called the entry active.
 */
describe('the settings group on a settings page', () => {
  function settingsEntry(): HTMLElement {
    const entry = [...screen.getByTestId('menu').querySelectorAll<HTMLElement>(':scope > a')]
      .find(link => link.getAttribute('href') === '/settings')
    if (!entry) throw new Error('no settings entry')
    return entry
  }

  it('opens and highlights when a settings page is reached after the shell mounted', async () => {
    const { router } = mountLayout()
    await router.isReady()
    expect(settingsEntry().dataset.open).toBe('false')
    expect(settingsEntry().dataset.active).toBe('false')

    await router.push('/settings/bandwidth')
    await nextTick()

    expect(settingsEntry().dataset.open).toBe('true')
    expect(settingsEntry().dataset.active).toBe('true')
  })

  it('is open and highlighted when the first route is a settings page', async () => {
    const { router } = mountLayout()
    await router.push('/settings/tools')
    await router.isReady()
    await nextTick()

    expect(settingsEntry().dataset.open).toBe('true')
    expect(settingsEntry().dataset.active).toBe('true')
  })

  it('stays closed once the reader closes it, and loses the highlight outside the settings', async () => {
    const { router } = mountLayout()
    await router.push('/settings/general')
    await router.isReady()
    await nextTick()

    await fireEvent.click(screen.getByTestId('toggle-group'))
    expect(settingsEntry().dataset.open).toBe('false')
    await router.push('/settings/network')
    await nextTick()
    expect(settingsEntry().dataset.open).toBe('false')

    await router.push('/downloads')
    await nextTick()
    expect(settingsEntry().dataset.active).toBe('false')
  })
})

describe('the sidebar header', () => {
  it('carries the name beside the logo and no tagline under it (owner, RD-120-53)', async () => {
    const { router } = mountLayout()
    await router.isReady()

    expect(screen.getByText('rDownloader')).toBeTruthy()
    expect(document.querySelector('[data-testid=sidebar] .eyebrow')).toBeNull()
  })
})
