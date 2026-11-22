// The relay between `page-files-main.js` in the page's world and the background (RD-1460-02).
//
// The page's world cannot reach the extension; this script, in the extension's world on the same
// page, can. It forwards a held file as base64 — Chrome's runtime messages carry JSON, not
// bytes — and posts the background's answer back. What a page could forge here gains it nothing:
// the background decides from the browser's own address of the frame, and the page could start
// the same download itself.
;(() => {
  const TYPE = 'rdownloader:page-file'
  const api = globalThis.browser ?? globalThis.chrome

  function base64(buffer) {
    const bytes = new Uint8Array(buffer)
    let binary = ''
    for (let index = 0; index < bytes.length; index += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(index, index + 0x8000))
    }
    return btoa(binary)
  }

  window.addEventListener('message', async (event) => {
    if (event.source !== window || event.data?.type !== TYPE || event.data.phase !== 'file') return
    const { id, name, mime, bytes } = event.data
    let taken = false
    try {
      const answer = await api.runtime.sendMessage({ type: TYPE, name: String(name ?? ''), mime: String(mime ?? ''), data: base64(bytes) })
      taken = answer?.taken === true
    } catch {
      taken = false
    }
    window.postMessage({ type: TYPE, phase: 'answer', id: String(id), taken }, location.origin)
  })
})()
