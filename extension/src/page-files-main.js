// Runs in the page's own world on a site the person allowed (RD-1460-02; the why: `page-files.js`).
//
// A classic script, not a module: a registered content script is not loaded as one. So the
// lists below are copies — of `BROWSER_ONLY_EXTENSIONS`/`BROWSER_ONLY_TYPES` in `intercept.js`
// and of `PAGE_FILE_MAX_BYTES` — and `test/page-files.test.mjs` holds them equal.
//
// It remembers each blob the page turns into an address, so the file is still at hand when the
// page revokes that address right after its click. A download of such an address under a
// browser-only name or type is held back and its bytes posted to the relay; on the answer
// "taken" nothing more happens, on any other answer — or none within `ANSWER_WAIT_MS` — the
// browser gets the file from a fresh address after all.
;(() => {
  const TYPE = 'rdownloader:page-file'
  const marker = Symbol.for(TYPE)
  if (globalThis[marker]) return
  Object.defineProperty(globalThis, marker, { value: true })

  const NAMES = ['.nzb', '.torrent', '.zip', '.dlc', '.ccf', '.rsdf', '.rdlinks', '.crawljob']
  const TYPES = [
    'application/x-nzb',
    'application/x-bittorrent',
    'application/zip',
    'application/x-zip-compressed',
    'application/dlc',
    'application/x-dlc',
    'application/x-ccf',
    'application/x-rsdf'
  ]
  const MAX_BYTES = 16 * 1024 * 1024
  const KEPT_BLOBS = 32
  const ANSWER_WAIT_MS = 30_000

  const create = URL.createObjectURL
  const revoke = URL.revokeObjectURL
  const click = HTMLAnchorElement.prototype.click
  const blobs = new Map()
  const waiting = new Map()

  URL.createObjectURL = function createObjectURL(object) {
    const url = create.call(URL, object)
    if (object instanceof Blob && object.size <= MAX_BYTES) {
      blobs.set(url, object)
      if (blobs.size > KEPT_BLOBS) blobs.delete(blobs.keys().next().value)
    }
    return url
  }

  URL.revokeObjectURL = function revokeObjectURL(url) {
    blobs.delete(String(url))
    return revoke.call(URL, url)
  }

  function candidate(anchor) {
    if (!(anchor instanceof HTMLAnchorElement) || !anchor.hasAttribute('download')) return null
    const blob = blobs.get(anchor.href)
    if (!blob) return null
    const name = String(anchor.getAttribute('download') ?? '').trim()
    const lower = name.toLowerCase()
    const type = String(blob.type ?? '').split(';')[0].trim().toLowerCase()
    if (!NAMES.some((extension) => lower.endsWith(extension)) && !TYPES.includes(type)) return null
    return { blob, name }
  }

  /** Gives the browser its download after all, from an address this script made itself. */
  function release(id, taken) {
    const found = waiting.get(id)
    if (!found) return
    waiting.delete(id)
    clearTimeout(found.timer)
    if (taken) return
    const url = create.call(URL, found.blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = found.name
    click.call(anchor)
    setTimeout(() => revoke.call(URL, url), ANSWER_WAIT_MS)
  }

  function hold(found) {
    const id = crypto.randomUUID()
    found.timer = setTimeout(() => release(id, false), ANSWER_WAIT_MS)
    waiting.set(id, found)
    found.blob.arrayBuffer().then(
      (bytes) => window.postMessage({ type: TYPE, phase: 'file', id, name: found.name, mime: found.blob.type, bytes }, location.origin, [bytes]),
      () => release(id, false)
    )
  }

  // A click on an anchor in the document, the page's script's `click()` included.
  window.addEventListener(
    'click',
    (event) => {
      const anchor = event.target instanceof Element ? event.target.closest('a') : null
      const found = anchor ? candidate(anchor) : null
      if (!found) return
      event.preventDefault()
      hold(found)
    },
    true
  )

  // An anchor that was never put into the document: its click reaches no listener above.
  HTMLAnchorElement.prototype.click = function () {
    if (!this.isConnected) {
      const found = candidate(this)
      if (found) {
        hold(found)
        return
      }
    }
    return click.call(this)
  }

  window.addEventListener('message', (event) => {
    if (event.source !== window || event.data?.type !== TYPE || event.data.phase !== 'answer') return
    release(String(event.data.id), event.data.taken === true)
  })
})()
