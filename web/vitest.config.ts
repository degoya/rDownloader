import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname
    }
  },
  test: {
    environment: 'jsdom',
    globals: true,
    // Vitest's default of 5 s is tight for tests that render every settings page in four
    // languages: one took 5.7 s on a GitHub runner (2026-09-25) and four more ran over locally
    // while other work loaded the machine. The limit catches a hang, not a slow render.
    testTimeout: 20_000
  }
})
