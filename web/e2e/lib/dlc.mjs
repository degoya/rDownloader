// A DLC container and a stand-in for the `dlcrypt` service it needs (RD-1260-11), so an end-to-end
// run imports one without the network. The format is the one `rd_collector::dlc` reads: base64 of
// the AES-128-CBC payload (the container key as key and IV, over the base64 of the XML), followed
// by an 88-character key blob; the service answers the blob with the container key, encrypted for
// pyLoad's published `pylo` identity. Node built-ins only, like `service.mjs`.
import { createCipheriv, randomBytes } from 'node:crypto'
import { createServer } from 'node:http'

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
