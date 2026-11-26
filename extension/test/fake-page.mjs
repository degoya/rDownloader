// One of the extension's pages, built from its own HTML (RD-1480-02).
//
// Just enough of a DOM for `popup.html`, `options.html` and `send-file.html`, because the extension
// is tested with nothing but Node: elements by id and by `data-i18n`, text, classes, listeners and
// clicks. A click runs inside the browser's gesture, so a handler that awaits before it asks for a
// permission is refused in Firefox as the real one refuses it.

import { readFileSync } from 'node:fs'

import { loadIn } from './fake-browser.mjs'

const VOID = ['meta', 'input', 'br', 'link']

class Element {
  constructor(tag) {
    this.tag = tag
    this.attributes = {}
    this.children = []
    this.dataset = {}
    this.listeners = {}
    this.hidden = false
    this.disabled = false
    this.checked = false
    this.value = ''
    this.className = ''
    this.text = ''
    this.classList = { add: () => {}, remove: () => {} }
  }

  get textContent() {
    return this.text + this.children.map((child) => child.textContent).join('')
  }

  set textContent(value) {
    this.text = String(value)
    this.children = []
  }

  setAttribute(name, value) {
    this.attributes[name] = value
  }

  append(...children) {
    this.children.push(...children)
  }

  replaceChildren(...children) {
    this.text = ''
    this.children = children
  }

  addEventListener(name, handler) {
    (this.listeners[name] ??= []).push(handler)
  }

  dispatch(name, event = {}) {
    for (const handler of this.listeners[name] ?? []) handler(event)
    this[`on${name}`]?.(event)
  }

  find(accepts, found = []) {
    for (const child of this.children) {
      if (accepts(child)) found.push(child)
      child.find(accepts, found)
    }
    return found
  }
}

function parse(html) {
  const document = new Element('#document')
  const open = [document]
  const body = html.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<!--[\s\S]*?-->/g, '')
  for (const [, closing, tag, attributes, text] of body.matchAll(/<(\/?)([a-z0-9]+)([^>]*)>([^<]*)/g)) {
    if (closing) {
      open.pop()
      continue
    }
    const element = new Element(tag)
    for (const [, name, value] of attributes.matchAll(/([a-z0-9-]+)(?:="([^"]*)")?/g)) element.attributes[name] = value ?? ''
    element.hidden = 'hidden' in element.attributes
    element.className = element.attributes.class ?? ''
    element.value = element.attributes.value ?? ''
    if ('data-i18n' in element.attributes) element.dataset.i18n = element.attributes['data-i18n']
    element.text = text.trim()
    open.at(-1).append(element)
    if (!VOID.includes(tag)) open.push(element)
  }
  return document
}

/**
 * Opens one of the extension's pages in this browser and loads its script. `clipboard` is what
 * the clipboard answers; a function that throws is a refused one.
 */
export async function openPage(browser, name, { clipboard = () => '' } = {}) {
  const document = parse(readFileSync(new URL(`../src/${name}.html`, import.meta.url), 'utf8'))
  const byId = (id) => document.find((element) => element.attributes.id === id)[0] ?? null
  const page = {
    document,
    byId,
    closed: false,
    all: (tag) => document.find((element) => element.tag === tag),
    status: () => [byId('status').textContent, byId('status').className],
    /** A person's click: on an element, or on the button with this text below `within`. */
    click: (element) => browser.gesture(() => element.dispatch('click')),
    button: (within, text) => byId(within).find((element) => element.tag === 'button' && element.textContent === text)[0],
    /** Types into a field, as the person does: the value, then its event. */
    type: (id, value, event = 'input') => {
      byId(id).value = value
      byId(id).dispatch(event)
    }
  }
  const root = document.find((element) => element.tag === 'html')[0]
  globalThis.document = {
    documentElement: root,
    title: document.find((element) => element.tag === 'title')[0]?.textContent ?? '',
    getElementById: byId,
    createElement: (tag) => new Element(tag),
    querySelectorAll: () => document.find((element) => element.dataset.i18n !== undefined)
  }
  globalThis.window = { close: () => { page.closed = true }, addEventListener: () => {} }
  Object.defineProperty(globalThis.navigator, 'clipboard', { configurable: true, value: { readText: async () => clipboard() } })
  page.root = root
  page.title = () => globalThis.document.title
  await loadIn(browser, `${name}.js`)
  return page
}
