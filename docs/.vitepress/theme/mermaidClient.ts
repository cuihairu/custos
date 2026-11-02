// Client-side mermaid renderer for ```mermaid fences.
//
// VitePress ships fences as <div class="language-mermaid"><code>…</code></div>
// and renders no SVG itself, so this module hydrates them in the browser:
//   - on first mount and on every SPA route change,
//   - on dark/light toggle (mermaid bakes theme colors at render time),
// reading the diagram source from the code element's text on FIRST render and
// stashing it on the block's dataset, because the render replaces the <pre>.
// Renders are serialized through a promise queue (mermaid.render mutates the
// DOM globally) and a parse error leaves the original code block in place.
import mermaid from 'mermaid'

let queue: Promise<void> = Promise.resolve()
let counter = 0

function currentTheme(): 'dark' | 'default' {
  return document.documentElement.classList.contains('dark') ? 'dark' : 'default'
}

async function renderAll(): Promise<void> {
  const blocks = Array.from(document.querySelectorAll('div.language-mermaid')) as HTMLElement[]
  if (!blocks.length) return

  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme: currentTheme(),
    flowchart: { useMaxWidth: true },
    er: { useMaxWidth: true },
  })

  for (const block of blocks) {
    if (!block.dataset.mermaidSource) {
      const code = block.querySelector('code')
      const source = code?.textContent ?? ''
      if (!source.trim()) continue
      block.dataset.mermaidSource = source
    }
    const source = block.dataset.mermaidSource
    const pre = block.querySelector('pre')
    if (!pre) continue
    // Already rendered for the current theme? Re-render only on theme change.
    if (block.dataset.mermaidTheme === currentTheme() && block.querySelector('svg')) continue
    try {
      const id = `mermaid-${++counter}`
      const { svg } = await mermaid.render(id, source)
      const parsed = new DOMParser().parseFromString(svg, 'image/svg+xml')
      const svgEl = parsed.documentElement
      if (parsed.querySelector('parsererror') || !svgEl.querySelector('svg,*')) {
        throw new Error('mermaid parse error')
      }
      pre.replaceChildren(svgEl)
      block.dataset.mermaidTheme = currentTheme()
    } catch (err) {
      // Leave the original code block in place; one bad diagram must not
      // take the page down.
      console.warn('[mermaid] failed to render diagram:', err)
    }
  }
}

function enqueue(): void {
  queue = queue.then(() => renderAll()).catch(() => {})
}

export function setupMermaid(): void {
  if (typeof window === 'undefined') return
  enqueue()
  // Dark/light toggle: <html> class flips, re-render with the new theme.
  new MutationObserver(enqueue).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class'],
  })
}
