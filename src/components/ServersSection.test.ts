// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick } from 'vue'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => true }))

import ServersSection from './ServersSection.vue'
import { SERVERS_KEY, loadProfiles, profiles } from '@/lib/servers'

async function settle(): Promise<void> {
  for (let i = 0; i < 4; i++) {
    await Promise.resolve()
    await nextTick()
  }
}

function mount(): HTMLElement {
  const host = document.createElement('div')
  document.body.append(host)
  createApp(ServersSection).mount(host)
  return host
}

function type(host: HTMLElement, label: string, value: string): void {
  const field = [...host.querySelectorAll('label')]
    .find((el) => el.textContent?.includes(label))
    ?.querySelector('input')
  if (!field) throw new Error(`no ${label} field`)
  field.value = value
  field.dispatchEvent(new Event('input'))
}

function button(host: HTMLElement, text: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((el) => el.textContent?.includes(text))
  if (!found) throw new Error(`no ${text} button`)
  return found
}

beforeEach(() => {
  localStorage.clear()
  profiles.value = []
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('Settings > Servers', () => {
  it('has no password field and says passwords are never saved', async () => {
    const host = mount()
    await settle()
    expect(host.querySelector('input[type="password"]')).toBeNull()
    expect(host.textContent).toContain('Passwords are never saved')
  })

  it('adds, edits and removes a server, and saves only the profile fields', async () => {
    const host = mount()
    type(host, 'Display name', 'Main file server')
    type(host, 'Server name', '\\\\fs01')
    type(host, 'User name', 'CORP\\sam')
    button(host, 'Add server').click()
    await settle()

    expect(host.textContent).toContain('Main file server')
    expect(host.textContent).toContain('fs01')
    expect(loadProfiles()).toMatchObject([{ server: 'fs01', user: 'CORP\\sam' }])
    expect(localStorage.getItem(SERVERS_KEY)).not.toContain('password')

    button(host, 'Edit').click()
    await settle()
    type(host, 'Display name', 'Renamed')
    button(host, 'Save changes').click()
    await settle()
    expect(loadProfiles()[0]?.name).toBe('Renamed')

    button(host, 'Remove').click()
    await settle()
    expect(loadProfiles()).toEqual([])
    expect(host.textContent).toContain('No servers are saved')
  })

  it('asks for a usable server name', async () => {
    const host = mount()
    type(host, 'Server name', 'C:\\Data')
    button(host, 'Add server').click()
    await settle()
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('server name')
    expect(loadProfiles()).toEqual([])
  })
})
