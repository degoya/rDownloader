import { readFileSync } from 'node:fs'
import { fileURLToPath, URL } from 'node:url'

import ui from '@nuxt/ui/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

import { uiTheme } from './src/uiTheme'

/**
 * Every icon the interface names is bundled at build time, none is fetched (RD-120-48).
 *
 * Without `clientBundle` Nuxt UI bundles only its own icons; the 160-odd the app names itself
 * were loaded from `api.iconify.design` at run time and were missing offline. The scan has to
 * read `.ts` as well as `.vue`, because the section, priority and kind maps that feed `:icon`
 * bindings live in TypeScript. `iconBundle.test.ts` fails when a name escapes it.
 */
export const iconClientBundle = {
  scan: {
    globInclude: ['src/**/*.{vue,ts}'],
    globExclude: ['node_modules', 'dist', '**/*.test.ts']
  },
  sizeLimitKb: 256
}

/**
 * The version the bundle is built as, from `package.json` (`scripts/set-version.sh` writes it).
 * The interface compares it with the service's to notice a page left open across an update
 * (RD-1120-16); `vitest.config.ts` defines the same.
 */
const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as { version: string }

export default defineConfig({
  define: { __RD_BUILD_VERSION__: JSON.stringify(version) },
  plugins: [
    vue(),
    ui({
      ui: uiTheme,
      icon: { clientBundle: iconClientBundle }
    })
  ],
  resolve: {
    alias: [
      // The offline build of the icon renderer: no API client, so no request can leave the
      // browser for an icon that did not make it into the bundle (see `src/iconifyOffline.ts`).
      { find: /^@iconify\/vue$/, replacement: fileURLToPath(new URL('./src/iconifyOffline.ts', import.meta.url)) },
      { find: '@', replacement: fileURLToPath(new URL('./src', import.meta.url)) }
    ]
  },
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            {
              // One chunk per language rather than one per catalogue file, so switching language
              // is a single request; English stays in the main chunk as the fallback (RD-140-27).
              // Any directory, so a language added to `languages.json` needs no change here.
              name(id) {
                const locale = /\/src\/locales\/([a-z]{2})\//.exec(id)?.[1]
                return locale && locale !== 'en' ? `locale-${locale}` : null
              }
            }
          ]
        }
      }
    }
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8710'
    }
  }
})
