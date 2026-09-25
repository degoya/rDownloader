/**
 * The shared component-test harness (RD-104-07).
 *
 * Every component test in this project used to repeat the same three things by hand: an
 * `createI18n` with a hand-picked set of catalogues, a fresh Pinia, and a map of Nuxt UI
 * stubs — because the real components resolve through `#imports`, which only exists inside a
 * Nuxt build. Twenty copies of that map drift apart, and a test then fails for a missing stub
 * rather than for the behaviour it was written to check.
 *
 * `vitest.config.ts` has no `setupFiles`, and this is deliberately not one: a setup file
 * cannot hand a test the catalogues it needs, and a global stub registry would hide which
 * component a test actually depends on. It is a module a test imports.
 */
import { render, type RenderResult } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import type { Component } from 'vue'
import { createI18n } from 'vue-i18n'

import { messageResolver } from '@/i18n/resolver'
import { DATETIME_FORMATS } from '@/i18n/formats'
import common from '@/locales/en/common.json'

/** Renders slot content, so what sits inside a Nuxt UI wrapper is reachable in the DOM. */
export const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }

/** A two-way bound input, for the wrappers that carry `modelValue`. */
export const modelInput = {
  props: ['modelValue'],
  emits: ['update:modelValue'],
  template:
    '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
}

/**
 * The Nuxt UI wrappers component tests need, rendered as the plain elements they stand for.
 * Buttons keep their label and `aria-label` so accessibility checks still see a name.
 */
export const uiStubs = {
  UAlert: { props: ['title', 'description'], template: '<div v-bind="$attrs">{{ title }}{{ description }}<slot /></div>' },
  UBadge: passthrough,
  UButton: {
    props: ['label', 'disabled', 'loading', 'ariaLabel'],
    template: '<button type="button" v-bind="$attrs" :disabled="disabled" :aria-label="ariaLabel">{{ label }}<slot /></button>'
  },
  UCard: passthrough,
  UCheckbox: {
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template: '<label>{{ label }}<input type="checkbox" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" /></label>'
  },
  UCollapsible: passthrough,
  UDashboardNavbar: passthrough,
  UDashboardPanel: { template: '<div><slot name="header" /><slot name="body" /></div>' },
  UDashboardSidebarCollapse: true,
  UDashboardToolbar: { template: '<div><slot /><slot name="left" /><slot name="right" /></div>' },
  /**
   * The dropdown rendered open: the trigger stays in place and every item becomes a real
   * button inside a marker, so a test can tell "beside the row" from "under the dots" and
   * can press an item without a portal.
   */
  UDropdownMenu: {
    props: ['items'],
    template:
      '<div><slot /><div data-menu-items>'
      + '<button v-for="item in (items ?? []).flat()" :key="item.label" type="button" :disabled="item.disabled" @click="item.onSelect?.()">{{ item.label }}</button>'
      + '</div></div>'
  },
  UFormField: passthrough,
  UFieldGroup: passthrough,
  UIcon: { template: '<span aria-hidden="true" />' },
  UInput: modelInput,
  /**
   * The combo box rendered open, with the search term where the real one keeps it: typing
   * changes the term, not the value, and only choosing an item or creating one writes through.
   * A stub that wrote every keystroke into the model would make the "create" option impossible
   * to reach whenever the offered items are derived from that model (RD-120-21).
   */
  UInputMenu: {
    inheritAttrs: false,
    props: {
      modelValue: {},
      items: {},
      createItem: { type: [Boolean, String, Object], default: false }
    },
    emits: ['update:modelValue', 'create'],
    data: () => ({ term: '' }),
    computed: {
      offered(this: { items?: string[] }) { return this.items ?? [] },
      unknown(this: { createItem?: unknown, term: string, offered: string[] }) {
        return Boolean(this.createItem) && this.term.length > 0 && !this.offered.includes(this.term)
      }
    },
    template:
      '<div><input v-bind="$attrs" :value="term || modelValue" @input="term = $event.target.value" />'
      + '<div data-menu-items>'
      + '<button v-for="item in offered" :key="item" type="button" @click="$emit(\'update:modelValue\', item)">{{ item }}</button>'
      + '<button v-if="unknown" type="button" data-create-item @click="$emit(\'create\', term)">{{ term }}</button>'
      + '</div></div>'
  },
  UModal: passthrough,
  UPopover: passthrough,
  UProgress: { template: '<div role="progressbar" v-bind="$attrs" />' },
  /**
   * Real radio inputs, each named by the label that wraps it.
   *
   * A stub that only rendered the labels would answer "is the first option preselected?" with
   * nothing, because the selection lives in the input's checkedness and nowhere else — which is
   * exactly the state RD-109-35 was about.
   */
  URadioGroup: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template:
      '<fieldset v-bind="$attrs"><label v-for="item in items" :key="item.value">'
      + '<input type="radio" :value="item.value" :checked="modelValue === item.value"'
      + ' @change="$emit(\'update:modelValue\', item.value)" />{{ item.label }}</label></fieldset>'
  },
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template:
      '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
  },
  USelectMenu: modelInput,
  UPagination: passthrough,
  USwitch: {
    props: ['modelValue', 'ariaLabel', 'disabled'],
    emits: ['update:modelValue'],
    template:
      '<button role="switch" v-bind="$attrs" :aria-label="ariaLabel" :aria-checked="modelValue" :disabled="disabled" @click="$emit(\'update:modelValue\', !modelValue)" />'
  },
  UTabs: { template: '<div><slot /><slot name="roots" /><slot name="categories" /><slot name="rules" /><slot name="hotfolders" /></div>' },
  UTextarea: modelInput,
  UTooltip: passthrough
}

/**
 * An i18n instance over the English catalogues a test names. `common` is always present
 * because the shared states — loading, failed — live there.
 */
export function createTestI18n(messages: Record<string, unknown> = {}, locale = 'en') {
  return createI18n({
    legacy: false,
    locale,
    messages: { [locale]: { common, ...messages } },
    // The application's resolver, not vue-i18n's: a backend code is itself dotted
    // (`server.codes.site_rules.structure`) and is stored as a literal key, so a test with the
    // default resolver would see every such message fall back to its key while the running
    // application resolves it.
    messageResolver,
    // The same shapes the application registers. Without them `d()` answers an unregistered
    // format with an empty string, so a timestamp a component renders would vanish under test
    // and the test would still pass.
    datetimeFormats: { [locale]: DATETIME_FORMATS }
  })
}

export interface MountOptions {
  /** Locale catalogues besides `common`, keyed by their namespace (`settings`, `network`, …). */
  messages?: Record<string, unknown>
  props?: Record<string, unknown>
  /** Extra or replacement stubs on top of `uiStubs`. */
  stubs?: Record<string, unknown>
  /** Extra plugins, for the rare test that needs a router. */
  plugins?: unknown[]
  /**
   * The locale the catalogues are registered under, `en` unless a test says otherwise.
   *
   * A test that has to prove a screen reads in somebody's own language needs the component
   * rendered in it, not the catalogue compared against itself (RD-120-15).
   */
  locale?: string
}

/**
 * Renders a component with a fresh Pinia, an i18n instance and the shared stubs.
 *
 * The Pinia is created per mount rather than per file, so a store a test writes to cannot
 * leak into the next one.
 */
export function mountComponent(component: Component, options: MountOptions = {}): RenderResult {
  setActivePinia(createPinia())
  return render(component, {
    props: options.props,
    global: {
      plugins: [createTestI18n(options.messages, options.locale), ...(options.plugins ?? [])] as never[],
      stubs: { ...uiStubs, ...options.stubs } as never
    }
  })
}
