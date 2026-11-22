// The relay between `page-files-main.js` in the page's world and the background (RD-1460-02).
//
// The page's world cannot reach the extension; this script, in the extension's world on the same
// page, can. It forwards a held file — base64 text, which the page script makes: Chrome's runtime
// messages carry JSON, not bytes, and Firefox lets a content script see no `ArrayBuffer` of the
// page (RD-1470-03) — and posts the background's answer back. What a page could forge here gains
// it nothing: the background decides from the browser's own address of the frame, and the page
// could start the same download itself.
;(() => {
  const TYPE = 'rdownloader:page-file'
  const api = globalThis.browser ?? globalThis.chrome

  window.addEventListener('message', async (event) => {
    if (event.source !== window || event.data?.type !== TYPE || event.data.phase !== 'file') return
    const { id, name, mime, data } = event.data
    let taken = false
    if (typeof data === 'string' && data.length > 0) {
      try {
        const answer = await api.runtime.sendMessage({ type: TYPE, name: String(name ?? ''), mime: String(mime ?? ''), data })
        taken = answer?.taken === true
      } catch (error) {
        console.warn('rDownloader: the file could not be handed over', error)
      }
    }
    window.postMessage({ type: TYPE, phase: 'answer', id: String(id), taken }, location.origin)
  })
})()
