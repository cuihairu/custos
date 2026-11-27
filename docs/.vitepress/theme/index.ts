import DefaultTheme from 'vitepress/theme'
import { inBrowser, useRoute, useData } from 'vitepress'
import { nextTick, watch } from 'vue'
import type { Theme } from 'vitepress'
import ArchitectureDiagram from './components/ArchitectureDiagram.vue'
import { setupMermaid } from './mermaidClient'
import { maybeRedirectHome } from './localeRedirect'

const theme: Theme = {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('ArchitectureDiagram', ArchitectureDiagram)
  },
  setup() {
    if (!inBrowser) return
    const route = useRoute()
    const { site } = useData()

    // Homepage-only language hint: a Chinese browser arriving on the English
    // root homepage gets sent to /zh/ once per session. All other paths and
    // any deliberate language switch are untouched (todo.md line 347 B).
    maybeRedirectHome({
      route,
      lang: navigator.language,
      base: site.value.base,
      storage: sessionStorage,
      location: window.location,
    })

    // Mermaid fences hydrate client-side: on first mount and on SPA navigation.
    watch(
      () => route.path,
      () => {
        nextTick(() => setupMermaid())
      },
      { immediate: true },
    )
  },
}

export default theme
import './custom.css'
