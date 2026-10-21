import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it } from 'vitest'

// The explorer script of the disk usage report, run against the page that report-core renders
// (see `explorer_fixture_matches_what_the_page_renders` in kinds.rs).
const fixture = readFileSync(
  resolve(process.cwd(), 'src-tauri/crates/report-core/tests/fixtures/explorer.html'),
  'utf8',
)

function load(): HTMLElement {
  const page = new DOMParser().parseFromString(fixture, 'text/html')
  const script = [...page.querySelectorAll('script')].find((s) =>
    s.textContent.includes("getElementById('x')"),
  )
  if (!script) {
    throw new Error('explorer script not found in the fixture')
  }
  document.body.innerHTML = page.body.innerHTML
  const host = document.getElementById('x')
  if (!host) {
    throw new Error('explorer container not found in the fixture')
  }
  host.hidden = false
  new Function(script.textContent)()
  return host
}

function button(host: HTMLElement, text: string, nth = 0): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].filter(
    (b) => (b.querySelector('.nm')?.textContent ?? b.textContent) === text,
  )
  const hit = found[nth]
  if (!hit) {
    throw new Error(`no button "${text}" (${nth}) in: ${host.textContent}`)
  }
  return hit
}

const filesShown = (host: HTMLElement): string[] =>
  [...host.querySelectorAll('.xfiles tbody tr td:first-child')].map((td) => td.textContent)

describe('disk usage report explorer', () => {
  let host: HTMLElement
  beforeEach(() => {
    host = load()
  })

  it('starts at the scanned folder with the biggest items and a folded row', () => {
    expect(host.querySelector('.xc')?.textContent).toBe('C:\\')
    const names = [...host.querySelectorAll('.xtree .xt > li .nm')].map((n) => n.textContent)
    expect(names).toEqual(['Users', 'Windows', 'pagefile.sys', '(2 smaller items)'])
    expect(host.querySelector('.xtree')?.textContent).toContain('600 bytes')
    expect(host.querySelector('.xtree')?.textContent).toContain('Showing the biggest items')
  })

  it('expands a folder lazily and keeps keyboard focus on the toggle', () => {
    const toggle = button(host, '\u25b8')
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    const tree = (): string => host.querySelector('.xtree')?.textContent ?? ''
    expect(tree()).not.toContain('Alice')
    toggle.click()
    expect(tree()).toContain('Alice')
    const opened = host.querySelector('[data-a="tg"][aria-expanded="true"]')
    expect(opened?.getAttribute('aria-label')).toBe('Collapse Users')
    expect(document.activeElement).toBe(opened)
  })

  it('never parses a file name as markup', () => {
    button(host, '\u25b8').click()
    expect(host.textContent).toContain('</script><b>x</b>.txt')
    expect(host.querySelector('b')).toBeNull()
    expect(host.querySelector('img')).toBeNull()
  })

  it('selects a folder: breadcrumb, and the type list narrows to files below it', () => {
    button(host, '\u25b8').click()
    button(host, 'Alice').click()
    expect(host.querySelector('.xc')?.textContent).toBe('C:\\\u203aUsers\u203aAlice')
    const types = [...host.querySelectorAll('.xtypes [data-a="ext"] .nm')].map((n) => n.textContent)
    expect(types).toEqual(['.mp4', '.txt'])
    expect(host.querySelector('.xtypes')?.textContent).toContain('top 200 per type')
    expect(host.querySelector('.xfiles h3')?.textContent).toBe('Largest files in C:\\Users\\Alice')
    expect(filesShown(host)).toEqual([
      'C:\\Users\\Alice\\Movies\\clip.mp4',
      'C:\\Users\\Alice\\notes.txt',
    ])
    // Back up through the breadcrumb.
    button(host, 'C:\\').click()
    expect(host.querySelector('.xc')?.textContent).toBe('C:\\')
    expect(filesShown(host)).toHaveLength(5)
  })

  it('filters by category and by type, and clears', () => {
    expect(filesShown(host)).toEqual([
      'C:\\Users\\Alice\\Movies\\clip.mp4',
      'C:\\Windows\\system.dll',
      'C:\\Users\\</script><b>x</b>.txt',
      'C:\\Users\\Alice\\notes.txt',
      'C:\\pagefile.sys',
    ])
    const video = host.querySelector<HTMLButtonElement>('[data-a="cat"]')
    expect(video?.getAttribute('aria-pressed')).toBe('false')
    video?.click()
    expect(host.querySelector('[data-a="cat"]')?.getAttribute('aria-pressed')).toBe('true')
    expect(filesShown(host)).toEqual(['C:\\Users\\Alice\\Movies\\clip.mp4'])
    expect(button(host, 'Showing Video \u2014 clear')).toBeTruthy()
    // A second click on the same category clears it.
    host.querySelector<HTMLButtonElement>('[data-a="cat"]')?.click()
    expect(filesShown(host)).toHaveLength(5)

    button(host, '.dll').click()
    expect(filesShown(host)).toEqual(['C:\\Windows\\system.dll'])
    expect(host.querySelector('.xfiles h3')?.textContent).toBe('Largest files: .dll')
    button(host, 'Showing .dll \u2014 clear').click()
    expect(filesShown(host)).toHaveLength(5)
    expect(host.querySelector('.chip')).toBeNull()
  })

  it('combines a folder with a filter', () => {
    button(host, '\u25b8').click()
    button(host, 'Alice').click()
    button(host, '.txt').click()
    expect(filesShown(host)).toEqual(['C:\\Users\\Alice\\notes.txt'])
    expect(host.querySelector('.xfiles h3')?.textContent).toBe(
      'Largest files: .txt in C:\\Users\\Alice',
    )
  })

  it('only offers real buttons, so everything works from the keyboard', () => {
    for (const el of host.querySelectorAll('[data-a]')) {
      expect(el.tagName).toBe('BUTTON')
      expect(el.getAttribute('type')).toBe('button')
    }
    expect(host.querySelector('[onclick]')).toBeNull()
  })
})
