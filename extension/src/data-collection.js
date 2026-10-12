// Firefox's optional data collection consent for the cookies that leave the browser (RD-1270-02).
//
// A session share, a session handover and a site's file hand-over send login cookies to the
// configured rDownloader. Firefox files them under `authenticationInfo`, which the Firefox build
// declares optional (`extension/build.mjs`), so it is asked for in the same request — the same
// click — as `cookies` and the site's origin, and checked again before any cookie is read. Without
// it no cookie leaves the browser in Firefox.
//
// Chrome has no data collection permissions, and its `permissions` API refuses the unknown
// `data_collection` key, so it is added only where the build declares it: what decides is the
// manifest the browser loaded, not a guess about the browser.

/** The category a cookie hand-over needs in Firefox. */
export const COOKIE_DATA = ['authenticationInfo']

/** Whether this build declares the optional cookie category (the Firefox build does). */
export function declaresCookieData(api) {
  try {
    const declared = api?.runtime?.getManifest?.()?.browser_specific_settings?.gecko?.data_collection_permissions
    return Array.isArray(declared?.optional) && COOKIE_DATA.every((category) => declared.optional.includes(category))
  } catch {
    return false
  }
}

/** A `permissions` request, with the cookie category added where the build declares it. */
export function withCookieData(api, request) {
  return declaresCookieData(api) ? { ...request, data_collection: [...COOKIE_DATA] } : request
}

/**
 * Whether cookies may leave: always where nothing is declared (Chrome), otherwise only while the
 * person holds the category — they can take it back in `about:addons` at any time.
 */
export async function holdsCookieData(api) {
  if (!declaresCookieData(api)) return true
  try {
    return Boolean(await api.permissions.contains({ data_collection: [...COOKIE_DATA] }))
  } catch {
    return false
  }
}
