/// <reference types="vite/client" />

/** `web/package.json`'s version, set by `define` in `vite.config.ts` and `vitest.config.ts`. */
declare const __RD_BUILD_VERSION__: string

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<Record<string, never>, Record<string, never>, unknown>
  export default component
}
