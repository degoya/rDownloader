/** `#build/ui-colors` as Nuxt UI's Vite plugin writes it: Tailwind's own palette, bundled. */
import colors from 'tailwindcss/colors'

export { colors }
export const loadColors = () => Promise.resolve(colors)
