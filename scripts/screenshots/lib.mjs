// What seed.mjs and capture.mjs share: the service's address, the work directory and the API
// calls with a session. Every path and port comes from the environment shots.sh sets; nothing
// here names a machine.
import fs from 'node:fs'
import path from 'node:path'

function required(name) {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is not set; run this through scripts/screenshots/shots.sh`)
  return value
}

export const WORK = required('RD_SHOTS_WORK')
export const DEMO = required('RD_SHOTS_DEMO')
export const BASE = `http://127.0.0.1:${process.env.RD_SHOTS_PORT ?? '18790'}`
export const MOCK = `http://127.0.0.1:${process.env.RD_SHOTS_MOCK_PORT ?? '18791'}`

const COOKIE_FILE = path.join(WORK, 'cookie.txt')
const PASSWORD_FILE = path.join(WORK, 'password')

/** The demo service's password: made by `shots.sh fresh`, kept in the work directory only. */
export function password() {
  if (!fs.existsSync(PASSWORD_FILE)) throw new Error(`no ${PASSWORD_FILE}; run shots.sh fresh first`)
  return fs.readFileSync(PASSWORD_FILE, 'utf8').trim()
}

let cookie = fs.existsSync(COOKIE_FILE) ? fs.readFileSync(COOKIE_FILE, 'utf8') : ''

export async function api(method, route, body, { ok = true } = {}) {
  const response = await fetch(BASE + route, {
    method,
    headers: {
      ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
      ...(cookie ? { cookie } : {}),
      origin: BASE
    },
    body: body !== undefined ? JSON.stringify(body) : undefined
  })
  const setCookie = response.headers.getSetCookie?.() ?? []
  if (setCookie.length) {
    cookie = setCookie.map(c => c.split(';')[0]).join('; ')
    fs.writeFileSync(COOKIE_FILE, cookie, { mode: 0o600 })
  }
  const text = await response.text()
  let data
  try { data = text ? JSON.parse(text) : null } catch { data = text }
  if (ok && !response.ok) {
    throw new Error(`${method} ${route} -> ${response.status}: ${text.slice(0, 500)}`)
  }
  return data
}

/** Sets the password on a fresh service, then signs in. */
export async function login() {
  const status = await api('GET', '/api/v1/auth/status')
  if (status?.setup_required) await api('POST', '/api/v1/auth/setup', { password: password() })
  await api('POST', '/api/v1/auth/login', { password: password() })
}

export const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
