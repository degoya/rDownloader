import { ref } from 'vue'

/**
 * Which groups of a flattened list are expanded (RD-106-12).
 *
 * Collapsing used to be a `ref` inside each package component, which works only while the
 * package owns its children. A virtualized list flattens the tree into one stream of rows and
 * filters it by what is open, so the answer has to live one level up — in the view — and be
 * the same answer for a package that is currently not rendered at all.
 *
 * Stored is the set of ids that differ from the default, so a list that is open by default
 * persists what the user closed and one that is closed by default persists what was opened.
 * That is the shape the queue already had under `rdownloader-open-packages`.
 */
export interface OpenSectionsOptions {
  /** `localStorage` key; omitted, the state lives for as long as the view does. */
  storageKey?: string
  /** What an id nobody has touched is. */
  defaultOpen?: boolean
}

export function useOpenSections(options: OpenSectionsOptions = {}) {
  const { storageKey, defaultOpen = false } = options

  function read(): Set<string> {
    if (!storageKey) return new Set()
    try {
      const raw = localStorage.getItem(storageKey)
      return new Set(raw ? JSON.parse(raw) as string[] : [])
    } catch {
      return new Set()
    }
  }

  /** Ids that differ from `defaultOpen`. */
  const flipped = ref<Set<string>>(read())

  function persist(): void {
    if (!storageKey) return
    try {
      localStorage.setItem(storageKey, JSON.stringify([...flipped.value]))
    } catch {
      // Storage unavailable (private mode, quota) — the in-memory state still answers.
    }
  }

  function isOpen(id: string): boolean {
    return flipped.value.has(id) ? !defaultOpen : defaultOpen
  }

  function set(id: string, open: boolean): void {
    const next = new Set(flipped.value)
    if (open === defaultOpen) next.delete(id)
    else next.add(id)
    flipped.value = next
    persist()
  }

  function toggle(id: string): void {
    set(id, !isOpen(id))
  }

  return { isOpen, set, toggle, flipped }
}
