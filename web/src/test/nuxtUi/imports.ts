/**
 * `#imports` and `#build/app.config` for the Nuxt UI components a test mounts for real
 * (`vitest.config.ts`): outside a Nuxt UI build neither exists. The least those components read —
 * no theme overrides, the icon names empty.
 */
const appConfig = { ui: { icons: { plus: '', minus: '', chevronUp: '', chevronDown: '' } } }

export const useAppConfig = () => appConfig

export default appConfig
