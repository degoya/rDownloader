/**
 * Whether two proxy addresses name the same proxy: scheme, host and port, as the server's
 * `same_proxy` compares them (RD-1200-06). A stored password is kept only for the same proxy;
 * any other address asks for it again. An address that does not parse names no proxy yet.
 */
export function sameProxyAddress(stored: string, entered: string): boolean {
  const before = parsed(stored)
  const after = parsed(entered)
  if (!before || !after) return false
  // `URL` drops the default port of `http` and `https`, as the server compares by it.
  return before.protocol === after.protocol
    && before.hostname === after.hostname
    && before.port === after.port
}

function parsed(value: string): URL | null {
  try {
    return new URL(value.trim())
  } catch {
    return null
  }
}
