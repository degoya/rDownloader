import type { MessageResolver, PathValue } from 'vue-i18n'

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/**
 * Resolves a message path, preferring a literal key over a nested walk at every level.
 *
 * Backend failure codes are themselves dotted (`server.codes.plugin.timeout`), and the
 * catalogues store them as literal keys inside `codes`. vue-i18n's default resolver only
 * splits on dots, so it would look for `codes → plugin → timeout` and find nothing. Trying
 * the remaining path as a literal key first makes both shapes work, which matters doubly
 * now that plugins ship their own `<slug>.<condition>` codes.
 */
export const messageResolver: MessageResolver = (obj, path) => {
  let node: unknown = obj
  let rest = String(path)
  while (rest.length > 0) {
    if (!isRecord(node)) return null
    if (rest in node) return node[rest] as PathValue
    const dot = rest.indexOf('.')
    if (dot === -1) return null
    const head = rest.slice(0, dot)
    if (!(head in node)) return null
    node = node[head]
    rest = rest.slice(dot + 1)
  }
  return node as PathValue
}
