// What crosses `runtime.sendMessage` and `storage` in each browser.
//
// Firefox clones structurally. Chrome carries JSON: a `Map`, a `Set` or an `ArrayBuffer` arrives
// as `{}`, a `Date` as a string, `NaN` as `null` — without an error (measured in Chromium 159,
// RD-1480-02). A fake that cloned structurally for both let such a payload pass every test, so the
// Chrome half throws where the two would differ. A member that is `undefined` is left out by JSON
// and reads the same on the other side, so that alone is no difference.

import { isDeepStrictEqual } from 'node:util'

function withoutUndefined(value) {
  if (Array.isArray(value)) return value.map(withoutUndefined)
  if (value === null || typeof value !== 'object' || Object.getPrototypeOf(value) !== Object.prototype) return value
  return Object.fromEntries(Object.entries(value).filter(([, member]) => member !== undefined).map(([key, member]) => [key, withoutUndefined(member)]))
}

/** The value as it arrives on the other side; throws for what does not arrive as it was sent. */
export function crossBoundary(target, value) {
  const cloned = structuredClone(value)
  if (target === 'firefox' || value === undefined) return cloned
  const text = JSON.stringify(value)
  const arrived = text === undefined ? undefined : JSON.parse(text)
  if (!isDeepStrictEqual(arrived, withoutUndefined(cloned))) throw new Error(`Chrome carries JSON: this payload does not arrive as it was sent (${text})`)
  return arrived
}
