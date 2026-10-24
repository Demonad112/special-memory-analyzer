// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick } from 'vue'

const reports = vi.hoisted(() => ({
  loadLogo: vi.fn(),
  pickLogo: vi.fn(),
  clearLogo: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => true }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }))
// jsdom has no matchMedia, which theme.ts reads at import time.
vi.mock('@/lib/theme', async () => ({
  themeChoice: (await import('vue')).ref('system'),
  setThemeChoice: vi.fn(),
}))
vi.mock('@/lib/reports', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/reports')>()),
  ...reports,
}))

import SettingsPage from './SettingsPage.vue'

async function settle(): Promise<void> {
  for (let i = 0; i < 4; i++) {
    await Promise.resolve()
    await nextTick()
  }
}

function mount(): HTMLElement {
  const host = document.createElement('div')
  document.body.append(host)
  createApp(SettingsPage).mount(host)
  return host
}

function button(host: HTMLElement, text: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((el) => el.textContent?.includes(text))
  if (!found) throw new Error(`no ${text} button`)
  return found
}

const png = 'data:image/png;base64,iVBO'

describe('SettingsPage company logo', () => {
  afterEach(() => {
    vi.resetAllMocks()
    document.body.innerHTML = ''
  })

  it('previews the saved logo, and Remove takes it away', async () => {
    reports.loadLogo.mockResolvedValue(png)
    reports.clearLogo.mockResolvedValue(undefined)
    const host = mount()
    await settle()
    expect(host.querySelector('img')?.getAttribute('src')).toBe(png)
    button(host, 'Remove').click()
    await settle()
    expect(reports.clearLogo).toHaveBeenCalledOnce()
    expect(host.querySelector('img')).toBeNull()
    expect(button(host, 'Choose').disabled).toBe(false)
  })

  it('shows why a file was refused and keeps the old logo', async () => {
    reports.loadLogo.mockResolvedValue(png)
    reports.pickLogo.mockRejectedValue('That file is not a PNG or JPEG image. Pick a .png or .jpg.')
    const host = mount()
    await settle()
    button(host, 'Change').click()
    await settle()
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('not a PNG or JPEG')
    expect(host.querySelector('img')?.getAttribute('src')).toBe(png)
  })

  it('keeps the logo when the dialog is cancelled, and shows a new pick', async () => {
    reports.loadLogo.mockResolvedValue(null)
    reports.pickLogo.mockResolvedValueOnce(null).mockResolvedValueOnce(png)
    const host = mount()
    await settle()
    expect(host.querySelector('img')).toBeNull()
    button(host, 'Choose').click()
    await settle()
    expect(host.querySelector('img')).toBeNull()
    button(host, 'Choose').click()
    await settle()
    expect(host.querySelector('img')?.getAttribute('src')).toBe(png)
  })

  it('disables the logo buttons until the load is back', async () => {
    let releaseLoad: (value: string | null) => void = () => undefined
    reports.loadLogo.mockReturnValue(
      new Promise((resolve) => {
        releaseLoad = resolve
      }),
    )
    const host = mount()
    await settle()
    expect(button(host, 'Choose').disabled).toBe(true)
    releaseLoad(png)
    await settle()
    expect(button(host, 'Change').disabled).toBe(false)
    expect(host.querySelector('img')?.getAttribute('src')).toBe(png)
  })
})
