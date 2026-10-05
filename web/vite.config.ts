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
        },
        // A card stands off the page by its ground, not by an outline on the page's own colour
        // (RD-1101-09); a card nested in another names `variant="outline"` itself. `overflow-clip`
        // instead of Nuxt UI's `overflow-hidden`: a hidden overflow makes the root a scroll
        // container, whose automatic minimum height in a flex column is 0, so in the panel body a
        // card shrank below its content, clipped it and left it out of the scroll height. A clip
        // keeps the rounded corners without that; `min-h-fit` was ignored by Firefox (RD-1110-17).
        card: { slots: { root: 'overflow-clip' }, defaultVariants: { variant: 'soft' } },
        // A number is typed, not stepped (RD-1110-10, `design.md`): only a small count shows its
        // plus and minus, by naming `increment` and `decrement` itself.
        inputNumber: { defaultVariants: { increment: false, decrement: false } },
        // An empty state is a dashed outline around Nuxt UI's own padding, one spacing for every
        // one of them (RD-1110-11); a ring cannot be dashed, so the outline is a border.
        empty: { slots: { root: 'border border-dashed border-muted' }, defaultVariants: { variant: 'naked' } },
        // A sub-section divider is the same muted hairline the hand-drawn `border-t` was.
        separator: { variants: { color: { neutral: { border: 'border-muted' } } } }
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
