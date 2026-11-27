// A DLC container and a stand-in for the `dlcrypt` service it needs (RD-1260-11), so an end-to-end
// run imports one without the network. The format is the one `rd_collector::dlc` reads: base64 of
// the AES-128-CBC payload (the container key as key and IV, over the base64 of the XML), followed
// by an 88-character key blob; the service answers the blob with the container key, encrypted for
// pyLoad's published `pylo` identity. Node built-ins only, like `service.mjs`.
//
// Since RD-1490-13 also the two ways a page offers a container in Chrome: built in the browser as
// a `blob:` download (`blobPage`), and behind a link that only a signed-in session may load
// (`startDlcSite`), which the extension hands over as its address with the session's cookies.
import { createCipheriv, randomBytes } from 'node:crypto'
import { createServer } from 'node:http'
import { networkInterfaces } from 'node:os'

const RC_KEY = Buffer.from('cb99b5cbc24db398')
const RC_IV = Buffer.from('9bc24cb995cb8db3')

const base64 = (text) => Buffer.from(text).toString('base64')

/** A container holding `links` in one package, and the answer the service gives for it. */
export function dlcContainer(links, packageName) {
  const key = randomBytes(16)
  const files = links
    .map((url) => `<file><url>${base64(url)}</url><filename>${base64(url.split('/').pop())}</filename></file>`)
    .join('')
  const xml = `<dlc><header><generator><app>e2e</app></generator></header><content><package name="${base64(packageName)}">${files}</package></content></dlc>`
  const cipher = createCipheriv('aes-128-cbc', key, key)
  const payload = Buffer.concat([cipher.update(base64(xml)), cipher.final()])
  const keyBlob = randomBytes(64).toString('base64')
  const wrap = createCipheriv('aes-128-cbc', RC_KEY, RC_IV).setAutoPadding(false)
  const wrapped = Buffer.concat([wrap.update(key), wrap.final()])
  return { container: `${payload.toString('base64')}${keyBlob}`, keyBlob, answer: `<rc>${wrapped.toString('base64')}</rc>` }
}

/** The stand-in service: answers each registered key blob with its key, anything else with 404. */
export async function startDlcService() {
  const answers = new Map()
  const server = createServer((request, response) => {
    const blob = new URL(request.url ?? '/', 'http://localhost').searchParams.get('data')
    const answer = blob ? answers.get(blob) : undefined
    response.writeHead(answer ? 200 : 404, { 'content-type': 'text/xml' })
    response.end(answer ?? '<rc></rc>')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    endpoint: `http://127.0.0.1:${server.address().port}/service.php`,
    register: ({ keyBlob, answer }) => answers.set(keyBlob, answer),
    close: () => new Promise((resolve) => server.close(resolve))
  }
}

/**
 * A page that builds its container in the browser, as some release pages do (RD-1460-02): a blob,
 * an anchor never put into the document, its click, and the address revoked at once.
 */
export function blobPage(container, name) {
  return `<!doctype html>
<meta charset="utf-8"><title>DLC test</title>
<button id="dlc">Download</button>
<script>
document.getElementById('dlc').addEventListener('click', () => {
  const url = URL.createObjectURL(new Blob([${JSON.stringify(container)}], { type: 'application/octet-stream' }))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = ${JSON.stringify(name)}
  anchor.click()
  URL.revokeObjectURL(url)
})
</script>`
}

/**
 * This machine's first IPv4 address on a private network, or null. The service fetches a handed
 * over address from there, where it refuses loopback: a page's address is a stranger's word.
 */
export function privateAddress() {
  for (const entries of Object.values(networkInterfaces())) {
    for (const entry of entries ?? []) {
      if (entry.family !== 'IPv4' || entry.internal) continue
      const [a, b] = entry.address.split('.').map(Number)
      if (a === 10 || (a === 172 && b >= 16 && b < 32) || (a === 192 && b === 168)) return entry.address
    }
  }
  return null
}

/**
 * A hoster with a login on `host`: its page sets a session cookie and shows what `serve` put there,
 * and each file `offer` names answers as a download only to a request carrying that cookie, as a
 * one-time link behind a login does. `refused` counts the requests that came without it.
 */
export async function startDlcSite(host) {
  const session = `sid=${randomBytes(8).toString('hex')}`
  const files = new Map()
  let page = ''
  let refused = 0
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? '/', 'http://localhost').pathname
    const file = files.get(path)
    if (!file) {
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'set-cookie': `${session}; Path=/` })
      response.end(page)
      return
    }
    if (!String(request.headers.cookie ?? '').split(/;\s*/).includes(session)) {
      refused += 1
      response.writeHead(403, { 'content-type': 'text/plain' })
      response.end('sign in first')
      return
    }
    response.writeHead(200, {
      'content-type': 'application/octet-stream',
      'content-disposition': `attachment; filename="${path.slice(1)}"`
    })
    response.end(file)
  })
  await new Promise((resolve) => server.listen(0, host, resolve))
  return {
    host,
    origin: `http://${host}:${server.address().port}`,
    serve: (html) => {
      page = html
    },
    offer: (name, content) => files.set(`/${name}`, Buffer.from(content)),
    refused: () => refused,
    close: () => new Promise((resolve) => server.close(resolve))
  }
}
