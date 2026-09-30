// A throwaway `rdownloader serve` and the client calls the end-to-end runs make against it
// (RD-180-12). Node built-ins only: the capture smoke imports this on runners that never install
// the web dependencies.
import { spawn } from 'node:child_process'
import { closeSync, mkdirSync, openSync, readFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { createServer as createNetServer } from 'node:net'
import { join } from 'node:path'

/**
 * The budgets of an end-to-end run. A step that takes longer fails the run: a service that needs
 * a minute to answer, or a link that reaches the LinkGrabber only after half a minute, is a
 * finding and not something to wait out.
 */
export const BUDGET_MS = {
  // A Windows runner's first start of a fresh 100 MB executable includes the virus scan.
  serviceStart: 45_000,
  agentStart: 20_000,
  linkArrival: 15_000
}

export function freePort() {
  return new Promise((resolve, reject) => {
    const server = createNetServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address()
      server.close(() => resolve(port))
    })
  })
}

/** Fails when `port` is taken, instead of letting a second service quietly answer the run. */
export function assertPortFree(port) {
  return new Promise((resolve, reject) => {
    const server = createNetServer()
    server.once('error', () => reject(new Error(`127.0.0.1:${port} is already in use`)))
    server.listen(port, '127.0.0.1', () => server.close(() => resolve()))
  })
}

/** Polls `probe` until it returns something truthy, within `budgetMs` or not at all. */
export async function within(budgetMs, what, probe, intervalMs = 250) {
  const started = Date.now()
  let lastError = null
  while (Date.now() - started < budgetMs) {
    try {
      const value = await probe()
      if (value) return { value, elapsedMs: Date.now() - started }
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs))
  }
  const cause = lastError ? ` (last error: ${lastError.message})` : ''
  throw new Error(`${what} took longer than its budget of ${budgetMs} ms${cause}`)
}

export function tail(path, lines = 60) {
  try {
    return readFileSync(path, 'utf8').split('\n').slice(-lines).join('\n')
  } catch {
    return `(no log at ${path})`
  }
}

/** Ends a child process and waits for it; a process that ignores the request is killed. */
export async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return
  const exited = new Promise((resolve) => child.once('exit', resolve))
  child.kill()
  const timer = setTimeout(() => child.kill('SIGKILL'), 10_000)
  await exited
  clearTimeout(timer)
}

/**
 * `env` without a session bus on Linux, so the OS keyring is out of reach.
 *
 * The service keeps its vault master key in the keyring when it finds one, and a throwaway
 * service on a desktop with a secret service would read the real installation's key, or mint one
 * there. Without a bus it keeps the key in the file beside its throwaway vault. `runtimeDir` is
 * set as well, because without an address the bus library falls back to `$XDG_RUNTIME_DIR/bus`.
 */
export function withoutSessionBus(env, runtimeDir) {
  if (process.platform !== 'linux') return env
  mkdirSync(runtimeDir, { recursive: true, mode: 0o700 })
  const isolated = { ...env, XDG_RUNTIME_DIR: runtimeDir }
  delete isolated.DBUS_SESSION_BUS_ADDRESS
  return isolated
}

/** Starts a child with its output in `logPath`; the returned `failed` names an early exit. */
export function launch(binary, args, { cwd, env, logPath }) {
  const log = openSync(logPath, 'a')
  const child = spawn(binary, args, { cwd, env: env ?? process.env, stdio: ['ignore', log, log] })
  closeSync(log)
  child.once('error', (error) => {
    child.launchError = error
  })
  return child
}

function exitedEarly(child, name, logPath) {
  if (child.launchError) throw new Error(`${name} could not be started: ${child.launchError.message}`)
  if (child.exitCode !== null || child.signalCode !== null) {
    throw new Error(`${name} exited with ${child.exitCode ?? child.signalCode}\n${tail(logPath)}`)
  }
}

/**
 * Starts `rdownloader serve` against an empty database in `workDir` and waits for its health
 * check. Plugins are neither bundled nor installed: the runs exercise the intake, not a hoster.
 */
export async function startService({ binary, port, workDir }) {
  mkdirSync(join(workDir, 'downloads'), { recursive: true })
  const logPath = join(workDir, 'service.log')
  const child = launch(
    binary,
    [
      'serve',
      '--database', join(workDir, 'rdownloader.sqlite3'),
      '--downloads', join(workDir, 'downloads'),
      '--plugin-root', join(workDir, 'plugins'),
      '--bundled-plugins', join(workDir, 'bundled-plugins'),
      '--listen', `127.0.0.1:${port}`
    ],
    { cwd: workDir, env: withoutSessionBus(process.env, join(workDir, 'run')), logPath }
  )
  const base = `http://127.0.0.1:${port}`
  const { value: health, elapsedMs } = await within(BUDGET_MS.serviceStart, 'the service start', async () => {
    exitedEarly(child, 'the service', logPath)
    const response = await fetch(`${base}/api/v1/health`)
    return response.ok ? response.json() : null
  })
  return { base, child, logPath, version: health.version, startMs: elapsedMs }
}

/** The web interface's side of the service: one administrator session, cookie by hand. */
export class Session {
  constructor(base) {
    this.base = base
    this.cookie = ''
  }

  async request(method, path, body) {
    const headers = {}
    if (this.cookie) headers.cookie = this.cookie
    if (body !== undefined) headers['content-type'] = 'application/json'
    const response = await fetch(`${this.base}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body)
    })
    const cookies = response.headers.getSetCookie?.() ?? []
    if (cookies.length) this.cookie = cookies.map((line) => line.split(';')[0]).join('; ')
    const text = await response.text()
    let json = null
    try {
      json = text ? JSON.parse(text) : null
    } catch {
      json = null
    }
    return { status: response.status, json, text }
  }

  async expect(status, method, path, body) {
    const result = await this.request(method, path, body)
    if (result.status !== status) {
      throw new Error(`${method} ${path} -> ${result.status} (expected ${status}): ${result.text.slice(0, 300)}`)
    }
    return result.json
  }

  /** Completes the setup of a fresh install and signs in. */
  async signIn(password) {
    await this.expect(200, 'POST', '/api/v1/auth/setup', { password })
    await this.expect(200, 'POST', '/api/v1/auth/login', { password })
  }

  /** A capture token, as Settings → Desktop client creates one. */
  async pairCapture(label) {
    const paired = await this.expect(201, 'POST', '/api/v1/capture/pair', { label })
    if (!paired?.bearer) throw new Error('the pairing answered without a bearer token')
    return paired.bearer
  }

  /** Waits for a LinkGrabber entry whose address contains `marker`. */
  async awaitCandidate(marker) {
    return within(BUDGET_MS.linkArrival, `the link ${marker} reaching the LinkGrabber`, async () => {
      const candidates = await this.expect(200, 'GET', '/api/v1/collector/candidates')
      return candidates.find((candidate) => String(candidate.url).includes(marker)) ?? null
    })
  }
}

/**
 * A local file host, so a handed-over link has an answer without the network: every GET or HEAD
 * under it is a small binary file.
 */
export async function startFileHost() {
  const server = createServer((request, response) => {
    const body = Buffer.alloc(4096, 0x5a)
    response.writeHead(200, {
      'content-type': 'application/octet-stream',
      'content-length': body.length
    })
    response.end(request.method === 'HEAD' ? undefined : body)
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve) => server.close(resolve))
  }
}

/** A short random marker that makes one run's links recognisable. */
export function marker(prefix) {
  return `${prefix}-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}
