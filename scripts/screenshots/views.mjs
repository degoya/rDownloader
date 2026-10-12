// Every view the screenshot tool takes (RD-1270-16). A view's name is the wiki's file name
// without `.png`; a new view takes the next free number of `images/screenshots/` when it is added
// here, so delivery never numbers anything. The group is the demo state it needs:
//
//   firstrun  a fresh service before setup, no session
//   wizard    the setup assistant after its password step
//   main      the demo state as `shots.sh seed` leaves it
//   pause     while the schedule pauses downloads (`seed.mjs pause-on`)
//   restart   while a switched-off plugin waits for a restart (`seed.mjs restart-on`)
//   held      the indexer drawer open while a check finds more hits (the mock's `more-music`)
//   signin    after setup, without a session
//   usenet    while a Usenet download holds connections (`seed.mjs usenet-on`: the mock's news
//             server behind a SOCKS5 profile, an NZB of three files, two of them running)
//
// `session: false` takes the view signed out, `fresh: true` in a browser context of its own.
// The wiki images the tool does not take — other applications, the media-server destinations,
// the colour-theme grid — are `by_hand` in targets.json.

/** Scrolls every scrollable panel of the page to `y`. */
export function panelScroll(page, y) {
  return page.evaluate((top) => {
    for (const el of document.querySelectorAll('main *, [id] *')) {
      const style = getComputedStyle(el)
      if (el.scrollHeight > el.clientHeight + 40 && /auto|scroll/.test(style.overflowY)) el.scrollTop = top
    }
  }, y)
}

/** Clicks the button named `button` in the row that shows `text`. */
async function rowButton(page, text, button) {
  await page.getByText(text, { exact: true }).first()
    .locator(`xpath=ancestor::*[.//button[@aria-label="${button}"]][1]`)
    .getByRole('button', { name: button }).first().click()
  await page.waitForTimeout(800)
}

async function center(page, selector, block = 'center') {
  await page.locator(selector).first().evaluate((el, b) => el.scrollIntoView({ block: b }), block)
  await page.waitForTimeout(500)
}

/** Opens the indexer review drawer at the foot of the LinkGrabber and picks a subscription. */
async function drawer(page, subscription, wait = 1500) {
  await page.getByRole('button', { name: 'Review', exact: true }).first().click()
  await page.waitForTimeout(1200)
  await page.getByRole('button', { name: subscription, exact: true }).last().click()
  await page.waitForTimeout(wait)
}

const view = (group, name, route, act, extra = {}) => ({ group, name, route, act, ...extra })
const settings = (name, route, act) => view('main', name, route, act)

export const VIEWS = [
  view('firstrun', '01-setup-wizard-password', '/downloads', null, { session: false }),
  view('wizard', '02-setup-wizard-pairing', '/downloads'),
  view('wizard', '03-setup-wizard-storage', '/downloads', async (page) => {
    await page.getByRole('button', { name: /^(Next|Skip)/ }).first().click()
  }),
  view('signin', '04-sign-in', '/downloads', null, { session: false }),

  view('main', '05-downloads', '/downloads'),
  view('main', '06-downloads-package-expanded', '/downloads', async (page) => {
    await page.getByRole('button', { name: 'Show files' }).first().click()
    await page.waitForTimeout(600)
    await page.getByRole('button', { name: 'Show details' }).first().click()
  }),
  view('main', '07-downloads-queue-status', '/downloads', page => panelScroll(page, 5000)),
  view('main', '08-linkgrabber', '/linkgrabber', async (page) => {
    await page.getByRole('checkbox', { name: 'Select package' }).first().click()
  }),
  view('main', '09-linkgrabber-add-links', '/linkgrabber', async (page) => {
    await page.getByRole('button', { name: 'Add links' }).first().click()
    await page.waitForTimeout(500)
    await page.getByRole('dialog').locator('textarea').first().fill([
      'https://files.example.org/open-movies/Glass.Half.2015.1080p.mkv',
      'https://files.example.org/open-movies/Agent.327.Operation.Barbershop.2017.1080p.mkv',
      'https://mirror.example.net/assets/nature-footage-4k-pack.part3.rar',
      'Links can be pasted inside any text — rDownloader picks out the addresses.'
    ].join('\n'))
    await page.getByRole('dialog').locator('input').first().fill('Blender Studio classics')
  }),
  view('main', '10-linkgrabber-indexer-drawer-cards', '/linkgrabber', page => drawer(page, 'Open Movie Project releases')),
  view('main', '11-linkgrabber-indexer-drawer-list', '/linkgrabber', page => drawer(page, 'Creative Commons music')),
  view('main', '12-streams', '/streams'),
  view('main', '13-streams-channels', '/streams', page => panelScroll(page, 650)),
  view('main', '14-subscriptions', '/subscriptions'),
  view('main', '15-subscriptions-hits', '/subscriptions', async (page) => {
    await page.getByRole('button', { name: 'Details' }).nth(1).click()
    await page.waitForTimeout(2000)
  }),
  view('main', '16-remote-jobs', '/remote-jobs'),
  view('main', '17-automation', '/automation'),
  view('main', '18-statistics', '/stats'),
  view('main', '19-logs', '/logs', async (page) => {
    // A debug window open, so the shot shows its end.
    if (!(await page.locator('[data-testid="log-debug-active"]').isVisible())) {
      await page.locator('[data-testid="log-debug-start"]').click()
      await page.locator('[data-testid="log-debug-active"]').waitFor({ timeout: 10000 })
    }
  }),
  view('main', '20-audit-log', '/audit'),
  view('main', '21-shortcuts-help', '/downloads', async (page) => {
    await page.keyboard.press('Shift+Slash')
    await page.waitForTimeout(600)
  }),
  view('main', '22-settings-overview', '/settings', page => panelScroll(page, 5000)),
  settings('23-settings-general', '/settings/general'),
  settings('24-settings-interface', '/settings/interface'),
  settings('25-settings-desktop', '/settings/clients'),
  settings('26-settings-routing-roots', '/settings/routing?tab=roots'),
  settings('27-settings-routing-categories', '/settings/routing?tab=categories'),
  settings('28-settings-routing-rules', '/settings/routing?tab=rules'),
  settings('29-settings-linkgrabber', '/settings/linkgrabber'),
  settings('30-settings-hotfolders', '/settings/hotfolders'),
  settings('31-settings-bandwidth', '/settings/bandwidth'),
  settings('32-settings-unattended', '/settings/unattended'),
  settings('33-settings-postprocess', '/settings/postprocess'),
  settings('34-settings-accounts', '/settings/accounts'),
  settings('35-settings-captcha', '/settings/captcha'),
  settings('36-settings-siterules', '/settings/siterules'),
  view('usenet', '37-settings-usenet', '/settings/usenet'),
  settings('38-settings-torrent', '/settings/torrent'),
  settings('39-settings-media', '/settings/media'),
  settings('40-settings-transfers', '/settings/transfers'),
  settings('41-settings-services', '/settings/services'),
  settings('42-settings-plugins', '/settings/plugins?tab=catalogue', async (page) => {
    await center(page, 'text=Services and plugins', 'start')
    await page.evaluate(() => {
      for (const el of document.querySelectorAll('*')) if (el.scrollTop > 0 && el.scrollHeight > el.clientHeight + 40) el.scrollTop -= 90
    })
  }),
  settings('43-settings-tools', '/settings/tools'),
  settings('44-settings-notifications', '/settings/notifications', async (page) => {
    // The ntfy destination opened in the form.
    await rowButton(page, 'Phone push (demo)', 'Edit')
    await panelScroll(page, 0)
  }),
  settings('45-settings-mcp', '/settings/clients?tab=api'),
  settings('46-settings-network', '/settings/network'),
  settings('47-settings-security', '/settings/security', async (page) => {
    await page.getByText('How long a sign-in lasts').first().evaluate(el => el.scrollIntoView({ block: 'center' }))
  }),
  settings('48-settings-backup', '/settings/backup'),
  settings('49-settings-system', '/settings/system'),

  view('main', '62-downloads-download-window', '/downloads', async (page) => {
    await rowButton(page, 'Community Distro 26.04', 'Package actions')
    await page.getByRole('menuitem', { name: /Download window/ }).first().click()
    await page.waitForTimeout(800)
  }),
  settings('63-settings-routing-category-window', '/settings/routing?tab=categories', async (page) => {
    await rowButton(page, 'Podcasts', 'Edit')
    await center(page, '[data-testid="download-window-editor"]')
  }),
  settings('64-settings-bandwidth-pause-downloads', '/settings/bandwidth?tab=profiles', async (page) => {
    await rowButton(page, 'Daytime — downloads paused', 'Edit')
    await center(page, '[data-testid="profile-pause-downloads"]')
  }),
  view('pause', '65-downloads-schedule-pause', '/downloads'),
  view('pause', '66-settings-bandwidth-paused', '/settings/bandwidth'),
  view('restart', '67-settings-system-restart', '/settings/system?tab=updates', async (page) => {
    await center(page, '[data-testid="restart-pending"]', 'start')
  }),
  settings('68-settings-system-cleanup', '/settings/system?tab=retention', async (page) => {
    await center(page, '[data-testid="update-backup-cleanup"]', 'start')
  }),
  view('main', '69-downloads-resolving-now', '/downloads', async (page) => {
    await rowButton(page, 'Blender open movies 4K', 'Show files')
    await rowButton(page, 'Nature footage 4K pack', 'Package actions')
    await page.getByRole('menuitem', { name: /Re-resolve/ }).first().click()
    await page.waitForTimeout(300)
    await rowButton(page, 'Nature footage 4K pack', 'Show files')
    await page.waitForTimeout(300)
  }, { fresh: true }), // a context of its own, so no other view's expanded rows carry over
  view('held', '70-linkgrabber-indexer-drawer-held', '/linkgrabber', async (page) => {
    await drawer(page, 'Creative Commons music', 1000)
    // The music subscription's check finds more while the list is open.
    await page.evaluate(async () => {
      const subs = await (await fetch('/api/v1/subscriptions')).json()
      const music = (subs.items ?? subs).find(s => s.name === 'Creative Commons music')
      await fetch(`/api/v1/subscriptions/${music.id}/poll`, { method: 'POST', headers: { origin: location.origin } })
    })
    await page.waitForTimeout(4000)
  }),
  view('usenet', '72-downloads-usenet-connections', '/downloads', async (page) => {
    await rowButton(page, 'Blender production files', 'Show files')
    await center(page, 'text=Blender production files', 'start')
  }, { fresh: true }),
  view('usenet', '73-settings-usenet-nntp-connections', '/settings/usenet', async (page) => {
    await center(page, '[data-testid="nntp-cap-hint"]')
  }),
  view('main', '74-automation-storage-threshold', '/automation', async (page) => {
    await rowButton(page, 'Tell Home Assistant the media library has room', 'Edit')
    await center(page, '[data-testid="automation-form"]', 'start')
  })
]

export const GROUPS = ['firstrun', 'wizard', 'main', 'pause', 'restart', 'held', 'signin', 'usenet']

/**
 * The views a `--views` list and a group select, in list order. An entry is a whole name or its
 * number (`05`); one that names no view throws, so a typo is never an empty run.
 */
export function selectViews(list, group) {
  const wanted = (list ?? '').split(',').map(s => s.trim()).filter(Boolean)
  for (const entry of wanted) {
    if (!VIEWS.some(v => v.name === entry || v.name.startsWith(`${entry}-`))) {
      throw new Error(`no view '${entry}'; node capture.mjs --list names them`)
    }
  }
  return VIEWS.filter(v => (!group || v.group === group)
    && (!wanted.length || wanted.some(e => v.name === e || v.name.startsWith(`${e}-`))))
}
