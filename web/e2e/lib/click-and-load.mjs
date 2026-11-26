// A hoster page with Click'n'Load and the two ends it can reach, shared by the runs of both
// browsers (RD-1260-10, RD-1480-04). Node built-ins only.
import { createCipheriv } from 'node:crypto'
import { createServer } from 'node:http'

/** The agent's port, the one every Click'n'Load page names. */
export const CNL_PORT = 9666
const KEY = 'abababababababababababababababab'

/** The links a page encrypts for Click'n'Load: AES-128-CBC with the key as its own IV. */
export function encrypt(text) {
  const key = Buffer.from(KEY, 'hex')
  const cipher = createCipheriv('aes-128-cbc', key, key)
  return Buffer.concat([cipher.update(text, 'utf8'), cipher.final()]).toString('base64')
}

/** A hoster page as they are written: the check script, then a form the button submits. */
export function hosterPage({ crypted, packageName, password }) {
  return `<!doctype html>
<meta charset="utf-8"><title>Click'n'Load test</title>
<script>var jdownloader = false</script>
<script src="http://127.0.0.1:${CNL_PORT}/jdcheck.js"></script>
<form id="cnl" action="http://127.0.0.1:${CNL_PORT}/flash/addcrypted2" method="post" target="sink">
  <input type="hidden" name="crypted" value="${crypted}">
  <input type="hidden" name="jk" value="function f(){ return '${KEY}'; }">
  <input type="hidden" name="passwords" value="${password}">
  <input type="hidden" name="package" value="${packageName}">
  <input type="hidden" name="source" value="http://hoster.test/">
  <button id="cnl-button" hidden>Click'n'Load</button>
</form>
<iframe name="sink"></iframe>
<script>if (window.jdownloader === true) document.getElementById('cnl-button').hidden = false</script>`
}

/** Serves whatever page the current step put there, with a login cookie when `cookie` is one. */
export async function startHoster({ cookie = null } = {}) {
  let page = ''
  const server = createServer((request, response) => {
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', ...(cookie ? { 'set-cookie': cookie } : {}) })
    response.end(page)
  })
  await new Promise((done) => server.listen(0, '127.0.0.1', done))
  return {
    port: server.address().port,
    serve: (html) => {
      page = html
    },
    close: () => new Promise((done) => server.close(done))
  }
}

/** A stand-in for the desktop agent on its port: it answers the check and keeps what it is sent. */
export async function startAgent() {
  const received = []
  const server = createServer((request, response) => {
    let body = ''
    request.on('data', (chunk) => {
      body += chunk
    })
    request.on('end', () => {
      if (request.url.startsWith('/flash/add')) received.push(body)
      response.writeHead(200, { 'access-control-allow-origin': '*' })
      response.end(request.url === '/jdcheck.js' ? "jdownloader=true; var version='agent';" : 'success')
    })
  })
  await new Promise((done) => server.listen(CNL_PORT, '127.0.0.1', done))
  return { received, close: () => new Promise((done) => server.close(done)) }
}
