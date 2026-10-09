// Capture-agent smoke on the operating system itself (RD-180-12): a fresh service, a capture
// token paired the way the web interface pairs one, `rdownloader-capture configure` and `run`,
// one link through the `rdownloader://` handler and one through Click'n'Load, both asserted in
// the LinkGrabber, and the autostart entry installed and removed again.
//
//   RD_E2E_SERVER=…/rdownloader RD_E2E_CAPTURE=…/rdownloader-capture node --test web/e2e/capture.e2e.mjs
//
// RD_E2E_OS_INTEGRATION=1 runs against the real user profile: the token goes to the system
// keyring and the autostart step registers and removes a real login entry. That is for a
// disposable CI runner. Without it the agent gets a throwaway profile (Linux only — Windows and
// macOS keep keyring and login items outside anything an environment variable can redirect) and
// the autostart step is skipped.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, before, test } from 'node:test'

import {
  BUDGET_MS,
  Session,
  freePort,
  launch,
  marker,
  startFileHost,
  startService,
  stop,
  tail,
  withoutSessionBus,
  within
} from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const CAPTURE = process.env.RD_E2E_CAPTURE
const OS_INTEGRATION = process.env.RD_E2E_OS_INTEGRATION === '1'
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-capture-'))
const RUN_KEY = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run'

let service
let session
let fileHost
let agent
let agentLog
let agentOrigin
let env

/** The capture agent's environment: the real profile, or a throwaway one on Linux. */
function agentEnvironment() {
  if (OS_INTEGRATION) return process.env
  if (process.platform !== 'linux') {
    throw new Error(
      'outside a disposable runner (RD_E2E_OS_INTEGRATION=1) this smoke runs on Linux only: ' +
        'the keyring and the login items of Windows and macOS cannot be redirected'
    )
  }
  const profile = join(WORK, 'profile')
  // No session bus: the keyring write fails and the agent keeps the token in its fallback file,
  // inside this profile, instead of replacing the capture token of whoever runs this.
  return withoutSessionBus(
    {
      ...process.env,
      HOME: profile,
      XDG_CONFIG_HOME: join(profile, '.config'),
      XDG_DATA_HOME: join(profile, '.local', 'share'),
      XDG_CACHE_HOME: join(profile, '.cache')
    },
    join(profile, 'run')
  )
}

function capture(args, input) {
  const result = spawnSync(CAPTURE, args, { env, input, encoding: 'utf8', timeout: 60_000 })
  if (result.error) throw result.error
  return result
}

function captureOk(args, input) {
  const result = capture(args, input)
  assert.equal(
    result.status,
    0,
    `rdownloader-capture ${args.join(' ')} exited with ${result.status}\n${result.stdout}\n${result.stderr}`
  )
  return result
}

/** Whether the login entry `autostart install` writes is in place, per operating system. */
function autostartRegistered() {
  if (process.platform === 'win32') {
    const query = spawnSync('reg.exe', ['query', RUN_KEY, '/v', 'rDownloader Capture'], { encoding: 'utf8' })
    return query.status === 0 && query.stdout.includes('wscript.exe')
  }
  if (process.platform === 'darwin') {
    const plist = join(homedir(), 'Library', 'LaunchAgents', 'org.rdownloader.capture.plist')
    return existsSync(plist) && spawnSync('plutil', ['-lint', plist]).status === 0
  }
  const config = env.XDG_CONFIG_HOME || join(homedir(), '.config')
  const unit = join(config, 'systemd', 'user', 'rdownloader-capture.service')
  const enabled = spawnSync('systemctl', ['--user', 'is-enabled', 'rdownloader-capture.service'], {
    env,
    encoding: 'utf8'
  })
  return existsSync(unit) && enabled.stdout.trim() === 'enabled'
}

before(async () => {
  assert.ok(SERVER, 'RD_E2E_SERVER names the rdownloader binary')
  assert.ok(CAPTURE, 'RD_E2E_CAPTURE names the rdownloader-capture binary')
  env = agentEnvironment()
  mkdirSync(WORK, { recursive: true })
  fileHost = await startFileHost()
  service = await startService({ binary: SERVER, port: await freePort(), workDir: join(WORK, 'service') })
  console.log(`service ${service.version} on ${service.base} after ${service.startMs} ms`)
  session = new Session(service.base)
  await session.signIn(`e2e-${marker('pw')}`)
})

after(async () => {
  await stop(agent)
  if (OS_INTEGRATION && CAPTURE) capture(['autostart', 'remove'])
  await stop(service?.child)
  await fileHost?.close()
  console.log(`logs in ${WORK}`)
})

test('the agent pairs, starts and takes links over both of its doors', { timeout: 180_000 }, async (t) => {
  const bearer = await session.pairCapture('E2E capture agent')
  captureOk(['configure', '--service', service.base, '--token-stdin'], `${bearer}\n`)

  const cnlPort = await freePort()
  agentOrigin = `http://127.0.0.1:${cnlPort}`
  agentLog = join(WORK, 'capture.log')
  // No `--service` and no token: the agent has to find both where `configure` put them.
  agent = launch(CAPTURE, ['run', '--no-tray', '--no-notifications', '--cnl-listen', `127.0.0.1:${cnlPort}`], {
    cwd: WORK,
    env,
    logPath: agentLog
  })
  const { elapsedMs } = await within(BUDGET_MS.agentStart, 'the capture agent start', async () => {
    if (agent.exitCode !== null) throw new Error(`the agent exited with ${agent.exitCode}\n${tail(agentLog)}`)
    const response = await fetch(`${agentOrigin}/flash`)
    return response.ok && (await response.text()) === 'JDownloader'
  })
  console.log(`capture agent listening on ${agentOrigin} after ${elapsedMs} ms`)

  await t.test('an rdownloader:// address reaches the LinkGrabber', async () => {
    const id = marker('scheme')
    const link = `${fileHost.origin}/${id}.bin`
    // Straight to the service with the pairing token `configure` stored (RD-1200-03): the agent's
    // own hand-over route and its `--agent` flag are gone.
    captureOk(['handle', `rdownloader://add?url=${encodeURIComponent(link)}`])
    const { value, elapsedMs: arrival } = await session.awaitCandidate(id)
    assert.equal(value.url, link)
    console.log(`scheme link in the LinkGrabber after ${arrival} ms`)
  })

  await t.test('a Click\'n\'Load form reaches the LinkGrabber', async () => {
    const id = marker('cnl')
    const link = `${fileHost.origin}/${id}.bin`
    const response = await fetch(`${agentOrigin}/flash/add`, {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ urls: link }).toString()
    })
    assert.equal(response.status, 200, `flash/add answered ${response.status}: ${await response.text()}`)
    const { value, elapsedMs: arrival } = await session.awaitCandidate(id)
    assert.equal(value.url, link)
    console.log(`Click'n'Load link in the LinkGrabber after ${arrival} ms`)
  })
})

test('autostart install registers the agent for the next login and remove takes it back', (t) => {
  if (!OS_INTEGRATION) {
    t.skip('needs RD_E2E_OS_INTEGRATION=1: it writes and removes a real login entry')
    return
  }
  assert.equal(autostartRegistered(), false, 'a capture autostart entry exists already; this run would remove it')
  captureOk(['autostart', 'install'])
  assert.equal(autostartRegistered(), true, 'autostart install left no login entry behind')
  captureOk(['autostart', 'remove'])
  assert.equal(autostartRegistered(), false, 'autostart remove left the login entry in place')
})
