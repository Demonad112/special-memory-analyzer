// @vitest-environment jsdom
import { invoke } from '@tauri-apps/api/core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref } from 'vue'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => true }))

import PathField from './PathField.vue'
import ServerShareList from './ServerShareList.vue'
import { profiles, type ServerProfile, type ServerShares } from '@/lib/servers'

const profile: ServerProfile = {
  id: 'p1',
  name: 'Main file server',
  server: 'fs01',
  user: 'CORP\\sam',
  notes: '',
}

const listed: ServerShares = {
  server: 'fs01',
  shares: [
    { name: 'Finance', comment: 'Finance team', hidden: false },
    { name: 'C$', comment: 'Default share', hidden: true },
  ],
  error: null,
  accessDenied: false,
}

async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve()
    await nextTick()
  }
}

function button(host: HTMLElement, text: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((el) => el.textContent?.includes(text))
  if (!found) throw new Error(`no ${text} button`)
  return found
}

beforeEach(() => {
  profiles.value = [profile]
})

afterEach(() => {
  vi.resetAllMocks()
  document.body.innerHTML = ''
  profiles.value = []
})

describe('ServerShareList', () => {
  function mount(picked: string[]): HTMLElement {
    const host = document.createElement('div')
    document.body.append(host)
    createApp(ServerShareList, {
      profile,
      pickLabel: 'Use',
      onPick: (path: string) => picked.push(path),
    }).mount(host)
    return host
  }

  it('lists shares with the hidden ones marked, and a share becomes a UNC path', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(listed)
    const picked: string[] = []
    const host = mount(picked)

    button(host, 'List shares').click()
    await settle()

    expect(invoke).toHaveBeenCalledWith('servers_list_shares', { server: 'fs01' })
    const rows = [...host.querySelectorAll('li')]
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining('Finance'),
      expect.stringMatching(/C\$.*hidden/s),
    ])
    const admin = rows[1]?.querySelector('button')
    admin?.click()
    expect(picked).toEqual(['\\\\fs01\\C$'])
  })

  it('offers the sign-in, with the profile user name, when access is denied', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      server: 'fs01',
      shares: [],
      error: 'Access denied for that account.',
      accessDenied: true,
    })
    const host = mount([])
    button(host, 'List shares').click()
    await settle()

    expect(host.textContent).toContain('Access denied for that account.')
    const details = host.querySelector('details')
    expect(details?.hasAttribute('open')).toBe(true)
    expect(host.textContent).toContain('\\\\fs01\\IPC$')
    const user = host.querySelector<HTMLInputElement>('details input[type="text"]')
    expect(user?.value).toBe('CORP\\sam')
    // No password is ever filled in.
    expect(host.querySelector<HTMLInputElement>('input[type="password"]')?.value).toBe('')
  })

  it('shows a plain error for an unreachable server without a sign-in', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      server: 'fs01',
      shares: [],
      error: 'The server could not be found. Check the server name.',
      accessDenied: false,
    })
    const host = mount([])
    button(host, 'List shares').click()
    await settle()
    expect(host.textContent).toContain('could not be found')
    expect(host.querySelector('details')).toBeNull()
  })

  it('a slow answer from an earlier click never fills the list', async () => {
    let first: (value: ServerShares) => void = () => undefined
    vi.mocked(invoke)
      .mockImplementationOnce(() => new Promise<ServerShares>((resolve) => (first = resolve)))
      .mockResolvedValueOnce({ ...listed, shares: [{ name: 'Newer', comment: '', hidden: false }] })
    const host = mount([])
    button(host, 'List shares').click()
    await settle()
    // The button is disabled while loading, so a second ask comes from a sign-in or remount;
    // call the command twice through the same lister by clicking after re-enabling.
    host.querySelector('button')?.removeAttribute('disabled')
    button(host, 'List shares').click()
    await settle()
    first({ ...listed, shares: [{ name: 'Older', comment: '', hidden: false }] })
    await settle()

    expect(host.textContent).toContain('Newer')
    expect(host.textContent).not.toContain('Older')
  })
})

describe('PathField server picker', () => {
  function mount(): { host: HTMLElement; path: ReturnType<typeof ref<string>> } {
    const path = ref('')
    const host = document.createElement('div')
    document.body.append(host)
    createApp(
      defineComponent(
        () => () =>
          h(PathField, {
            label: 'Folder',
            hint: '',
            modelValue: path.value,
            'onUpdate:modelValue': (value: string) => (path.value = value),
          }),
      ),
    ).mount(host)
    return { host, path }
  }

  it('has no Servers button until a server is saved', async () => {
    profiles.value = []
    const { host } = mount()
    await settle()
    expect(host.textContent).not.toContain('Servers')
  })

  it('fills the folder with the picked share', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(listed)
    const { host, path } = mount()
    button(host, 'Servers').click()
    await settle()
    button(host, 'List shares').click()
    await settle()
    host.querySelector<HTMLElement>('ul button')?.click()
    await settle()

    expect(path.value).toBe('\\\\fs01\\Finance')
    expect(host.querySelector('ul')).toBeNull()
  })
})
