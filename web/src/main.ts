import '@fontsource-variable/ibm-plex-sans'
import '@fontsource-variable/jetbrains-mono'
import './assets/main.css'

import ui from '@nuxt/ui/vue-plugin'
import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from './App.vue'
import { i18n, setLocale, currentLocale } from './i18n'
import { router } from './router'

// The installed plugins' own translations are not merged here: that endpoint sits behind the
// sign-in, and the sign-in screen has no plugin strings to translate. The session store fetches
// them the moment a session exists.
setLocale(currentLocale())

// Registered after load so it never competes with the first render. A failure is ignored on
// purpose: the app works without it, and an install prompt is not worth an error dialog.
if ('serviceWorker' in navigator && window.isSecureContext) {
  window.addEventListener('load', () => {
    void navigator.serviceWorker.register('/sw.js').catch(() => undefined)
  })
}

const app = createApp(App)

// Vue's default is to log a render error and drop the failing subtree, which is how an invalid
// select item could silently leave a dropdown empty instead of announcing itself. Keep the
// logging explicit so such a failure is recognisable as a bug rather than an empty control.
app.config.errorHandler = (error, _instance, info) => {
  console.error(`[rDownloader] unhandled component error (${info})`, error)
}

app
  .use(createPinia())
  .use(router)
  .use(i18n)
  .use(ui)
  .mount('#app')
