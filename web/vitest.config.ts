import { readFileSync } from 'node:fs'

import vue from '@vitejs/plugin-vue'
import { configDefaults, defineConfig } from 'vitest/config'

// The two files the VM pool cannot run (RD-1120-08): one loads the real vite config, whose
// Tailwind plugin registers module hooks no VM context offers, and one replaces
// `window.location`, which jsdom defines as non-configurable on its window — inside a VM context
// the global itself. They run in the default pool beside the rest.
const OUTSIDE_THE_VM = ['src/iconBundle.test.ts', 'src/components/settings/SettingsSessions.test.ts']

// The build's version, as `vite.config.ts` defines it (RD-1120-16).
const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as { version: string }

export default defineConfig({
  define: { __RD_BUILD_VERSION__: JSON.stringify(version) },
  plugins: [vue()],
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
      // The Nuxt UI build modules, for the tests that mount a real Nuxt UI component
      // (`src/test/inputNumber.test.ts`, `src/components/SearchableSelect.test.ts`); every other
      // test renders the stubs of `mount.ts`.
      '#imports': new URL('./src/test/nuxtUi/imports.ts', import.meta.url).pathname,
      '#build/app.config': new URL('./src/test/nuxtUi/imports.ts', import.meta.url).pathname,
      '#build/ui/input-number': new URL('./src/test/nuxtUi/inputNumberTheme.ts', import.meta.url).pathname,
      // The real select menu of `SearchableSelect.test.ts` and the search field inside it (RD-1180-02).
      '#build/ui/select-menu': new URL('./src/test/nuxtUi/selectMenuTheme.ts', import.meta.url).pathname,
      '#build/ui/input': new URL('./src/test/nuxtUi/selectMenuTheme.ts', import.meta.url).pathname
    }
  },
  test: {
    environment: 'jsdom',
    globals: true,
    // Through Vite, so the aliases above reach the Nuxt UI runtime's own imports of `#imports`;
    // Node would resolve them against the package and fail.
    server: { deps: { inline: [/@nuxt\/ui\/dist\/runtime\//] } },
    // Vitest's default of 5 s is tight for tests that render every settings page in four
    // languages: one took 5.7 s on a GitHub runner (2026-09-25) and four more ran over locally
    // while other work loaded the machine. The limit catches a hang, not a slow render. Hooks get
    // the same: `beforeAll(loadEveryLocale)` ran past the default 10 s in the 1.5.1 release check.
    testTimeout: 20_000,
    hookTimeout: 20_000,
    projects: [
      // jsdom created once per worker instead of once per file — 231 times and 37 % of the
      // tracked time before — while every file still runs in a context of its own, so `vi.mock`
      // and the stores stay per file. A worker past the memory limit is replaced.
      // `isolate: false` was rejected: 133 files mock modules, 36 set the active Pinia.
      {
        extends: true,
        test: {
          name: 'vm',
          pool: 'vmThreads',
          vmMemoryLimit: '1024MB',
          exclude: [...configDefaults.exclude, ...OUTSIDE_THE_VM]
        }
      },
      { extends: true, test: { name: 'forks', include: OUTSIDE_THE_VM } }
    ]
  }
})
