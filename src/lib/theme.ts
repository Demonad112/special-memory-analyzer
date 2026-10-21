import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ref } from 'vue'

export type ThemeChoice = 'system' | 'light' | 'dark'

const STORAGE_KEY = 'analyzer-theme'
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)')

export function resolveTheme(choice: ThemeChoice, prefersDark: boolean): 'light' | 'dark' {
  if (choice === 'system') {
    return prefersDark ? 'dark' : 'light'
  }
  return choice
}

export function loadThemeChoice(): ThemeChoice {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    return saved === 'light' || saved === 'dark' ? saved : 'system'
  } catch {
    return 'system'
  }
}

export const themeChoice = ref<ThemeChoice>(loadThemeChoice())

function paint(): void {
  document.documentElement.dataset.theme = resolveTheme(themeChoice.value, darkQuery.matches)
}

export function setThemeChoice(choice: ThemeChoice): void {
  themeChoice.value = choice
  try {
    localStorage.setItem(STORAGE_KEY, choice)
  } catch {
    // Storage blocked: the choice still applies for this run.
  }
  paint()
  if (isTauri()) {
    // Mica tints from the window theme; null follows Windows.
    void getCurrentWindow().setTheme(choice === 'system' ? null : choice)
  }
}

export function initTheme(): void {
  darkQuery.addEventListener('change', paint)
  setThemeChoice(themeChoice.value)
}
