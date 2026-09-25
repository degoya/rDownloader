import { fileURLToPath, URL } from 'node:url'

import ui from '@nuxt/ui/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

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

export default defineConfig({
  plugins: [
    vue(),
    ui({
      ui: {
        colors: {
          primary: 'signal',
          secondary: 'cyan',
          neutral: 'slate',
          warning: 'amber',
          error: 'coral'
        }
      },
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
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8710'
    }
  }
})
