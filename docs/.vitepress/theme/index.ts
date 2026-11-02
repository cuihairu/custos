import DefaultTheme from 'vitepress/theme'
import { inBrowser, useRoute } from 'vitepress'
import { nextTick, watch } from 'vue'
import type { Theme } from 'vitepress'
import ArchitectureDiagram from './components/ArchitectureDiagram.vue'
import { setupMermaid } from './mermaidClient'

const theme: Theme = {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('ArchitectureDiagram', ArchitectureDiagram)
  },
  setup() {
    if (!inBrowser) return
    const route = useRoute()
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
