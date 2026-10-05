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
import { useFileUpload } from '@nuxt/ui/composables/useFileUpload'
import { render, type RenderResult } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { defineComponent, toRef, type Component, type ComponentPublicInstance, type PropType } from 'vue'
import { createI18n } from 'vue-i18n'

import { messageResolver } from '@/i18n/resolver'
import { DATETIME_FORMATS } from '@/i18n/formats'
import common from '@/locales/en/common.json'

/** Renders slot content, so what sits inside a Nuxt UI wrapper is reachable in the DOM. */
export const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }

/** A two-way bound input, for the wrappers that carry `modelValue`. */
const modelInput = {
  props: ['modelValue'],
  emits: ['update:modelValue'],
  template:
    '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
}

/**
 * The file field with Nuxt UI's own `useFileUpload` behind it (RD-1110-12): the picker is a real
 * hidden `<input type="file">` carrying the field's attributes, as in the real one, so a test
 * chooses with a `change` on it; without a default slot the area is the drop zone,
 * `[data-file-drop]`, and takes a `drop` the way the real one does — `accept` included. The
 * field is told its files through `update:modelValue`, appended to `modelValue` when `multiple`.
 */
export const fileUpload = defineComponent({
  inheritAttrs: false,
  props: {
    modelValue: { type: [Object, Array] as PropType<File | File[] | null>, default: null },
    accept: { type: String, default: '*' },
    multiple: Boolean,
    reset: Boolean,
    dropzone: { type: Boolean, default: true },
    interactive: { type: Boolean, default: true },
    label: { type: String, default: '' },
    description: { type: String, default: '' },
    // Styling the real field takes as props; here they would land on the input.
    icon: { type: String, default: undefined },
    size: { type: String, default: undefined },
    layout: { type: String, default: undefined },
    preview: { type: Boolean, default: true }
  },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    const { open, inputRef, dropzoneRef } = useFileUpload({
      accept: toRef(props, 'accept'),
      multiple: toRef(props, 'multiple'),
      reset: toRef(props, 'reset'),
      dropzone: props.dropzone,
      onUpdate: files => emit('update:modelValue', props.multiple
        ? [...(Array.isArray(props.modelValue) ? props.modelValue : []), ...files]
        : files[0] ?? null)
    })
    // The real field hands the composable its input as a component, whose element is `$el`.
    const setInput = (element: unknown) => {
      inputRef.value = element ? { $el: element } as unknown as ComponentPublicInstance : undefined
    }
    return { open, dropzoneRef, setInput }
  },
  template:
    '<div><slot :open="open"><div ref="dropzoneRef" data-file-drop :role="interactive ? \'button\' : undefined" @click="interactive && open()">'
    + '{{ label }} {{ description }}<slot name="actions" :open="open" /></div></slot>'
    + '<input :ref="setInput" type="file" hidden :accept="accept" :multiple="multiple" v-bind="$attrs" /></div>'
})

/**
 * The Nuxt UI wrappers component tests need, rendered as the plain elements they stand for.
 * Buttons keep their label and `aria-label` so accessibility checks still see a name.
 */
export const uiStubs = {
  /** Title and description as lines of their own, and the slots a notice with content fills (RD-1110-11). */
  UAlert: {
    props: ['title', 'description'],
    template:
      '<div v-bind="$attrs"><div v-if="title">{{ title }}</div><slot name="title" /><div v-if="description">{{ description }}</div>'
      + '<slot name="description" /><slot /><slot name="actions" /></div>'
  },
  /** The fallback text the real one shows where it has no picture; an icon is decoration. */
  UAvatar: { props: ['icon', 'text'], template: '<span v-bind="$attrs">{{ text }}</span>' },
  UBadge: passthrough,
  UButton: {
    props: ['label', 'disabled', 'loading', 'ariaLabel'],
    template: '<button type="button" v-bind="$attrs" :disabled="disabled" :aria-label="ariaLabel">{{ label }}<slot /></button>'
  },
  /** The element the card renders as, with its three regions in their real order (RD-180-22). */
  UCard: {
    props: ['as'],
    template: '<component :is="as ?? \'div\'" v-bind="$attrs"><slot name="header" /><slot /><slot name="footer" /></component>'
  },
  /** The dot alone, as the real one renders it standalone: no text, so nothing to read out. */
  UChip: { props: ['color', 'show'], template: '<span data-chip :data-color="color"><span v-if="show !== false" data-chip-dot /></span>' },
  UCheckbox: {
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template: '<label>{{ label }}<input type="checkbox" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" /></label>'
  },
  /**
   * A fieldset of real checkboxes under its legend, as Reka's group renders it, so a test finds
   * each chip by role and name and reads the pressed state from `checked` (RD-150-11).
   */
  UCheckboxGroup: {
    props: ['modelValue', 'items', 'legend', 'valueKey', 'labelKey', 'disabled'],
    emits: ['update:modelValue'],
    methods: {
      valueOf(this: { valueKey?: string }, item: unknown): unknown {
        return item !== null && typeof item === 'object' ? (item as Record<string, unknown>)[this.valueKey ?? 'value'] : item
      },
      labelOf(this: { labelKey?: string }, item: unknown): unknown {
        return item !== null && typeof item === 'object' ? (item as Record<string, unknown>)[this.labelKey ?? 'label'] : item
      },
      toggle(this: { modelValue?: unknown[], $emit: (event: string, value: unknown[]) => void }, value: unknown, on: boolean): void {
        const current = (this.modelValue ?? []).filter(entry => entry !== value)
        this.$emit('update:modelValue', on ? [...current, value] : current)
      }
    },
    template:
      '<fieldset v-bind="$attrs" :disabled="disabled"><legend v-if="legend">{{ legend }}</legend>'
      + '<label v-for="item in items" :key="String(valueOf(item))">'
      + '<input type="checkbox" :checked="(modelValue ?? []).includes(valueOf(item))"'
      + ' @change="toggle(valueOf(item), $event.target.checked)" />{{ labelOf(item) }}</label></fieldset>'
  },
  /**
   * Every item a header button with `aria-expanded`, and its body rendered only while open —
   * the real one unmounts a closed panel, so a test can tell open from closed by presence.
   */
  UAccordion: {
    props: ['items', 'modelValue', 'type'],
    emits: ['update:modelValue'],
    methods: {
      keyOf(item: { value?: string }, index: number): string {
        return item.value ?? String(index)
      },
      isOpen(this: { modelValue?: string | string[], keyOf: (item: object, index: number) => string }, item: object, index: number): boolean {
        const key = this.keyOf(item, index)
        return Array.isArray(this.modelValue) ? this.modelValue.includes(key) : this.modelValue === key
      },
      toggle(this: { modelValue?: string | string[], type?: string, isOpen: (item: object, index: number) => boolean, keyOf: (item: object, index: number) => string, $emit: (event: string, value: unknown) => void }, item: object, index: number): void {
        const key = this.keyOf(item, index)
        const open = this.isOpen(item, index)
        if (this.type === 'multiple') {
          const current = Array.isArray(this.modelValue) ? this.modelValue : []
          this.$emit('update:modelValue', open ? current.filter(entry => entry !== key) : [...current, key])
        } else {
          this.$emit('update:modelValue', open ? undefined : key)
        }
      }
    },
    template:
      '<div v-bind="$attrs"><div v-for="(item, index) in items" :key="keyOf(item, index)" data-accordion-item :data-state="isOpen(item, index) ? \'open\' : \'closed\'">'
      + '<button type="button" :aria-expanded="isOpen(item, index)" @click="toggle(item, index)">'
      + '<slot :item="item" :index="index" :open="isOpen(item, index)">{{ item.label }}</slot>'
      + '<slot name="trailing" :item="item" :index="index" :open="isOpen(item, index)" /></button>'
      + '<div v-if="isOpen(item, index)" data-accordion-body><slot name="body" :item="item" :index="index" :open="true" /></div>'
      + '</div></div>'
  },
  /**
   * The trigger in place and the content rendered only while open, as the real one unmounts a
   * closed panel; controlled through `open` or left to itself from `defaultOpen` (RD-180-22).
   */
  UCollapsible: {
    props: { open: { type: Boolean, default: undefined }, defaultOpen: Boolean },
    emits: ['update:open'],
    data(this: { defaultOpen: boolean }) {
      return { inner: this.defaultOpen }
    },
    computed: {
      shown(this: { open?: boolean, inner: boolean }): boolean {
        return this.open ?? this.inner
      }
    },
    methods: {
      toggle(this: { shown: boolean, inner: boolean, $emit: (event: string, value: boolean) => void }): void {
        this.inner = !this.shown
        this.$emit('update:open', this.inner)
      }
    },
    template:
      '<div v-bind="$attrs" :data-state="shown ? \'open\' : \'closed\'"><div @click="toggle"><slot :open="shown" /></div>'
      + '<div v-if="shown" data-collapsible-content><slot name="content" /></div></div>'
  },
  /** Title, description and the action buttons in the real order, each action a button by its label (RD-1110-11). */
  UEmpty: {
    props: ['title', 'description', 'icon', 'actions'],
    template:
      '<div v-bind="$attrs"><p v-if="title">{{ title }}</p><p v-if="description">{{ description }}</p><slot name="description" />'
      + '<button v-for="action in actions ?? []" :key="action.label" type="button" @click="action.onClick">{{ action.label }}</button>'
      + '<slot name="actions" /><slot name="body" /><slot name="footer" /></div>'
  },
  USeparator: { template: '<div role="separator" v-bind="$attrs" />' },
  /**
   * Every column's header and cell slot, the way the real table hands them `row.original`, with
   * the table's `ui.base` and each column's `meta.class` where the real one puts them; a column
   * without a slot shows its `header` and its `accessorKey` value.
   */
  UTable: {
    props: ['data', 'columns', 'ui'],
    template:
      '<table v-bind="$attrs" :class="ui?.base"><thead><tr><th v-for="column in columns" :key="column.id ?? column.accessorKey" :class="column.meta?.class?.th">'
      + '<slot :name="`${column.id ?? column.accessorKey}-header`">{{ typeof column.header === \'string\' ? column.header : \'\' }}</slot></th></tr></thead>'
      + '<tbody><tr v-for="(item, index) in data" :key="index" data-row><td v-for="column in columns" :key="column.id ?? column.accessorKey" :class="column.meta?.class?.td">'
      + '<slot :name="`${column.id ?? column.accessorKey}-cell`" :row="{ original: item }">{{ column.accessorKey ? item[column.accessorKey] : \'\' }}</slot></td></tr></tbody></table>'
  },
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
  /**
   * The label wraps the control, which gives it the name the real field gives it through `for`;
   * a field without a label renders its slot bare, as the real one names nothing either.
   */
  UFormField: {
    props: ['label'],
    template: '<div v-bind="$attrs"><label v-if="label">{{ label }}<slot /></label><slot v-else /></div>'
  },
  UFieldGroup: passthrough,
  UIcon: { template: '<span aria-hidden="true" />' },
  UInput: modelInput,
  /**
   * The number field as Reka's renders it — a text input with the role `spinbutton` — handing
   * its model a number, and `undefined` for an emptied field as the real one does (RD-1110-10).
   * It neither clamps nor reads a decimal comma; `utils/numberInput.test.ts` runs the real parser.
   */
  UInputNumber: {
    props: ['modelValue', 'formatOptions', 'stepSnapping', 'increment', 'decrement'],
    emits: ['update:modelValue'],
    template:
      '<input type="text" role="spinbutton" v-bind="$attrs" :value="modelValue ?? \'\'" '
      + '@input="$emit(\'update:modelValue\', $event.target.value.trim() === \'\' ? undefined : Number($event.target.value))" />'
  },
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
  /** An anchor to where `to` points; the router is the real one's business, not the test's. */
  ULink: { props: ['to'], template: '<a v-bind="$attrs" :href="to"><slot /></a>' },
  UModal: passthrough,
  /**
   * The card with its title and description, and — as the real one does with `to` — an empty
   * link over it named by the title, which takes the attributes the card was given.
   */
  UPageCard: {
    inheritAttrs: false,
    props: ['to', 'title', 'description'],
    template:
      '<div><slot name="leading" /><div>{{ title }}</div><div>{{ description }}</div><slot />'
      + '<a v-if="to" v-bind="$attrs" :href="to" :aria-label="title"><span aria-hidden="true" /></a></div>'
  },
  UPopover: passthrough,
  /** Named by its percentage unless the caller names it, as Reka's `ProgressRoot` does. */
  UProgress: { props: ['modelValue'], template: '<div role="progressbar" :aria-label="`${modelValue ?? 0}%`" v-bind="$attrs" />' },
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
    props: ['modelValue', 'ariaLabel', 'label', 'disabled'],
    emits: ['update:modelValue'],
    template:
      '<button type="button" role="switch" v-bind="$attrs" :aria-label="ariaLabel ?? label" :aria-checked="modelValue" :disabled="disabled" @click="$emit(\'update:modelValue\', !modelValue)" />'
  },
  /**
   * A tab per item and every item's slot rendered, the inactive ones `hidden` — what the real one
   * does with `:unmount-on-hide="false"`, which every settings page with sub-tabs uses. Hidden
   * content stays in the DOM but out of role queries, as it is out of reach for a screen reader.
   */
  UTabs: {
    props: ['items', 'modelValue'],
    emits: ['update:modelValue'],
    template:
      '<div><div role="tablist"><span v-for="item in items ?? []" :key="item.value" role="tab" tabindex="0"'
      + ' :aria-selected="item.value === modelValue" @click="$emit(\'update:modelValue\', item.value)">{{ item.label }}{{ item.badge ?? \'\' }}</span></div>'
      + '<slot /><div v-for="item in items ?? []" :key="item.value" role="tabpanel" :data-tab="item.value"'
      + ' :hidden="modelValue !== undefined && item.value !== modelValue"><slot :name="item.slot ?? item.value" :item="item" /></div></div>'
  },
  UTextarea: modelInput,
  UTooltip: passthrough,
  UFileUpload: fileUpload
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

interface MountOptions {
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
