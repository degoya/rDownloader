import { ref, toValue, type MaybeRefOrGetter } from 'vue'

/**
 * Which groups of a flattened list are expanded (RD-106-12).
 *
 * Collapsing used to be a `ref` inside each package component, which works only while the
 * package owns its children. A virtualized list flattens the tree into one stream of rows and
 * filters it by what is open, so the answer has to live one level up — in the view — and be
 * the same answer for a package that is currently not rendered at all.
 *
 * Stored is an explicit id → open map of what the user opened or closed (RD-1170-01). It used to
 * be the set of ids that differ from the default, which inverts its meaning once the default is a
 * setting that can change; with the map, a touched group keeps its state and an untouched one
 * follows the default. An array under the key is the shape before that: the queue's open ids,
 * written while its default was closed.
 */
interface OpenSectionsOptions {
  /** `localStorage` key; omitted, the state lives for as long as the view does. */
  storageKey?: string
  /** What an id nobody has touched is; a ref or getter follows a setting that changes. */
  defaultOpen?: MaybeRefOrGetter<boolean>
}

export function useOpenSections(options: OpenSectionsOptions = {}) {
  const { storageKey, defaultOpen = false } = options

  function read(): Map<string, boolean> {
    if (!storageKey) return new Map()
    try {
      const raw = localStorage.getItem(storageKey)
      const parsed: unknown = raw ? JSON.parse(raw) : null
      if (Array.isArray(parsed)) return new Map(parsed.filter(id => typeof id === 'string').map(id => [id, true]))
      if (!parsed || typeof parsed !== 'object') return new Map()
      return new Map(Object.entries(parsed).filter((entry): entry is [string, boolean] => typeof entry[1] === 'boolean'))
    } catch {
      return new Map()
    }
  }

  /** The groups the user opened or closed, and which of the two. */
  const touched = ref<Map<string, boolean>>(read())

  function persist(): void {
    if (!storageKey) return
    try {
      localStorage.setItem(storageKey, JSON.stringify(Object.fromEntries(touched.value)))
    } catch {
      // Storage unavailable (private mode, quota) — the in-memory state still answers.
    }
  }

  function isOpen(id: string): boolean {
    return touched.value.get(id) ?? toValue(defaultOpen)
  }

  function setAll(ids: Iterable<string>, open: boolean): void {
    const next = new Map(touched.value)
    for (const id of ids) next.set(id, open)
    touched.value = next
    persist()
  }

  function set(id: string, open: boolean): void {
    setAll([id], open)
  }

  function toggle(id: string): void {
    set(id, !isOpen(id))
  }

  /** Forgets the groups that are gone, so the stored map does not grow for ever. */
  function prune(known: Iterable<string>): void {
    const keep = new Set(known)
    if ([...touched.value.keys()].every(id => keep.has(id))) return
    touched.value = new Map([...touched.value].filter(([id]) => keep.has(id)))
    persist()
  }

  return { isOpen, set, setAll, toggle, prune, touched }
}
