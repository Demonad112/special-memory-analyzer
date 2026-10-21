import { beforeEach, describe, expect, it } from 'vitest'

// jsdom has no matchMedia; theme.ts reads it at import time.
window.matchMedia = (query: string): MediaQueryList =>
  ({ matches: false, media: query, addEventListener: () => undefined }) as unknown as MediaQueryList

const { loadThemeChoice, resolveTheme, setThemeChoice } = await import('./theme')

describe('theme', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('follows the OS only for the system choice', () => {
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })

  it('falls back to system for missing or unknown saved values', () => {
    expect(loadThemeChoice()).toBe('system')
    localStorage.setItem('analyzer-theme', 'purple')
    expect(loadThemeChoice()).toBe('system')
  })

  it('saves the choice and paints the document', () => {
    setThemeChoice('dark')
    expect(loadThemeChoice()).toBe('dark')
    expect(document.documentElement.dataset.theme).toBe('dark')
  })
})
