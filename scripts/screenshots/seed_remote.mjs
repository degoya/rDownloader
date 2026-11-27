// seed.mjs `remote` (RD-1490-13): the remote-jobs view's rows. The mock answers Real-Debrid's API
// over TLS (remote_mock.py) behind the proxy profile `base` made global; this trusts the mock's
// throwaway CA through the custom CA setting (Settings > Network), adds a demo Real-Debrid
// account — its token a placeholder — and submits the mock's magnets: two running, one queued at
// the provider, one finished, one failed. The `plugins` phase installed the realdebrid service,
// whose remote-job plugin realdebrid-torrents is. The finished job's LinkGrabber package is
// removed again, so the LinkGrabber views stay as they were.
import fs from 'node:fs'
import path from 'node:path'
import { api, sleep, MOCK, WORK } from './lib.mjs'

const LABEL = 'Real-Debrid (demo)'
const PROVIDER = 'realdebrid'
const WAIT_MS = 180_000
const SETTLED = ['working', 'ready', 'failed']

/** Every state in `jobs` but `submitting`, with a ready and a failed one among them. */
const settled = jobs => jobs.length > 0 && jobs.every(job => job.state !== 'submitting') &&
  SETTLED.every(state => jobs.some(job => job.state === state))

export async function seedRemoteJobs(patchSettings) {
  const caFile = path.join(WORK, 'tls', 'ca.pem')
  if (!fs.existsSync(caFile)) throw new Error(`no ${caFile}: the mock makes it at its start (see logs/mock.log)`)
  const ca = fs.readFileSync(caFile, 'utf8').trim()
  const providers = await api('GET', '/api/v1/remote-jobs/providers')
  if (!providers.includes(PROVIDER)) throw new Error(`no remote-job plugin for ${PROVIDER} (${providers.join(', ') || 'none'}): seed plugins first`)

  const settings = await api('GET', '/api/v1/settings')
  if (!(settings.custom_ca_pem ?? '').includes(ca)) {
    await patchSettings({ custom_ca_pem: [settings.custom_ca_pem?.trim(), ca].filter(Boolean).join('\n') + '\n' })
    console.log('custom CA: the mock\'s demo CA trusted')
  }

  const accounts = await api('GET', '/api/v1/accounts')
  let account = (accounts.items ?? accounts).find(a => a.label === LABEL)
  if (!account) {
    account = await api('POST', '/api/v1/accounts', { provider: PROVIDER, label: LABEL, username: null, credential_mode: 'api_key',
      secret: 'not-a-real-token', cookies: null, proxy_profile_id: null, enabled: true })
    console.log('account', LABEL, account.id)
  }

  const response = await fetch(MOCK + '/remote-jobs/magnets.json')
  if (!response.ok) throw new Error(`${MOCK}/remote-jobs/magnets.json -> ${response.status}`)
  // Submitted in the mock's order; the list shows the newest first.
  for (const { name, magnet } of await response.json()) {
    const { job, already_running } = await api('POST', `/api/v1/accounts/${account.id}/remote-jobs`, { magnet })
    console.log('remote job', name, job.state, already_running ? '(already there)' : '')
    await sleep(1500)
  }

  let jobs = []
  for (const deadline = Date.now() + WAIT_MS; Date.now() < deadline; await sleep(3000)) {
    jobs = (await api('GET', '/api/v1/remote-jobs')).filter(job => job.account_id === account.id)
    if (settled(jobs)) break
  }
  for (const job of jobs) console.log(job.state.padEnd(16), String(job.progress_permille ?? '').padStart(5), job.source_name, job.code ?? '')
  if (!settled(jobs)) throw new Error(`the jobs did not reach their states in ${WAIT_MS / 1000} s`)
  for (const job of jobs.filter(job => job.state === 'ready' && job.package_id)) {
    await api('DELETE', `/api/v1/collector/packages/${job.package_id}`).catch(e => { if (!/ -> 404:/.test(e.message)) throw e })
    console.log('removed the LinkGrabber package of', job.source_name)
  }
}
