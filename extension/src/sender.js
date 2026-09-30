// Who may talk to the background. No browser globals here: the captcha and handover modules
// import it, and their tests hand in a fake `runtime` of their own.

/**
 * Whether a runtime message comes from a page of this extension - the popup, or the popup opened
 * as a tab. A content script always carries `sender.tab`; the popup as a popup carries none, and
 * the popup opened as a tab carries one, so the URL decides that case: it has to be one of our
 * own pages. Every message that acts on the service or on a grant is answered only for these.
 */
export function fromOwnPage(runtime, sender) {
  if (!sender || sender.id !== runtime.id) return false
  if (!sender.tab) return true
  const own = runtime.getURL('')
  return typeof sender.url === 'string' && own !== '' && sender.url.startsWith(own)
}
