import type { RenderBuiltAssetUrl } from 'vite'

/**
 * How the build writes the address of a file it emitted, so that it holds under a mount point
 * nobody knew at build time (RD-1250-09).
 *
 * The bundle is built once, at the root, and embedded in the binary; a reverse proxy may put it
 * under `/rd` or `/downloads`. The server prefixes the absolute references of `index.html` and
 * the manifest as it serves them (`crates/rd-api/src/static_assets.rs`), but it cannot do the
 * same inside the JavaScript and CSS without a search-and-replace that would also rewrite the
 * application's own strings. So those two never carry an absolute path into `assets/`:
 *
 * - **JavaScript** builds the address at run time from the base the server injected as
 *   `window.__RD_BASE__` (the one `basePath.ts` reads) — Vite's chunk and CSS preloads among
 *   them, which asked for it at the root and got a 404 behind a proxy that passes only `/rd/`.
 * - **CSS** refers to its fonts and pictures relative to itself: every stylesheet lies in
 *   `assets/` beside them, wherever that is mounted.
 * - **HTML** keeps the absolute path the server prefixes.
 */
export const renderBuiltUrl: RenderBuiltAssetUrl = (filename, { hostType }) => {
  if (hostType === 'js') {
    return { runtime: `(window.__RD_BASE__||"")+${JSON.stringify(`/${filename}`)}` }
  }
  if (hostType === 'css') {
    return { relative: true }
  }
  return undefined
}
