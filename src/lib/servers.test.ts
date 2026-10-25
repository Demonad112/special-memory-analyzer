// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import {
  SERVERS_KEY,
  addProfile,
  createShareLister,
  loadProfiles,
  parseProfiles,
  profiles,
  removeProfile,
  serializeProfiles,
  serverName,
  sharePath,
  signInShare,
  updateProfile,
  type ServerShares,
} from './servers'

beforeEach(() => {
  localStorage.clear()
  profiles.value = []
})

describe('server names', () => {
  it('takes the server out of whatever was typed', () => {
    expect(serverName('fs01')).toBe('fs01')
    expect(serverName('\\\\fs01')).toBe('fs01')
    expect(serverName('  \\\\fs01\\Finance\\x ')).toBe('fs01')
    expect(serverName('//fs01/Finance')).toBe('fs01')
    expect(serverName('fs01.corp.local')).toBe('fs01.corp.local')
  })

  it('refuses names a server cannot have', () => {
    for (const bad of ['', '  ', '\\\\', 'C:\\Data', 'a b', 'fs*01', '\\\\?\\C:\\x', '\\\\.\\x']) {
      expect(serverName(bad), bad).toBe('')
    }
  })

  it('builds share paths, including admin shares', () => {
    expect(sharePath('fs01', 'Finance')).toBe('\\\\fs01\\Finance')
    expect(sharePath('\\\\fs01', 'C$')).toBe('\\\\fs01\\C$')
    expect(signInShare('fs01')).toBe('\\\\fs01\\IPC$')
  })
})

describe('server profiles', () => {
  it('round-trips through storage in order', () => {
    const a = addProfile({
      name: 'Main file server',
      server: '\\\\fs01',
      user: 'CORP\\sam',
      notes: 'D: is Finance',
    })
    const b = addProfile({ name: '', server: 'nas', user: '', notes: '' })
    expect(a?.server).toBe('fs01')
    expect(b?.name).toBe('nas')

    const loaded = loadProfiles()
    expect(loaded).toEqual([a, b])
    expect(profiles.value).toEqual(loaded)
  })

  it('refuses a profile without a usable server', () => {
    expect(addProfile({ name: 'x', server: 'C:\\Data', user: '', notes: '' })).toBeNull()
    expect(profiles.value).toEqual([])
  })

  it('edits and removes a profile', () => {
    const profile = addProfile({ name: 'a', server: 'fs01', user: '', notes: '' })!
    expect(updateProfile(profile.id, { name: 'b', server: 'fs02', user: ' u ', notes: 'n' })).toBe(
      true,
    )
    expect(loadProfiles()[0]).toMatchObject({
      id: profile.id,
      name: 'b',
      server: 'fs02',
      user: 'u',
    })
    expect(updateProfile('missing', { name: 'b', server: 'fs02', user: '', notes: '' })).toBe(false)
    expect(updateProfile(profile.id, { name: 'b', server: '', user: '', notes: '' })).toBe(false)
    removeProfile(profile.id)
    expect(loadProfiles()).toEqual([])
  })

  it('never saves a password, even when one is put on a profile', () => {
    const profile = addProfile({ name: 'a', server: 'fs01', user: 'sam', notes: '' })!
    // A later change that gives the type a secret would still be dropped by the serializer.
    const sneaky = { ...profile, password: 'hunter2', secret: 'x' }
    const saved = serializeProfiles([sneaky])
    expect(saved).not.toContain('password')
    expect(saved).not.toContain('hunter2')
    expect(Object.keys(JSON.parse(saved)[0] as object).sort()).toEqual([
      'id',
      'name',
      'notes',
      'server',
      'user',
    ])
    expect(localStorage.getItem(SERVERS_KEY)).not.toContain('password')
  })

  it('drops a password found in older or edited storage when it loads', () => {
    localStorage.setItem(
      SERVERS_KEY,
      JSON.stringify([{ id: '1', name: 'a', server: 'fs01', user: '', notes: '', password: 'x' }]),
    )
    const [profile] = loadProfiles()
    expect(profile).toBeDefined()
    expect(Object.keys(profile!)).not.toContain('password')
    expect(serializeProfiles(loadProfiles())).not.toContain('password')
  })

  it('skips broken, duplicate and unusable entries', () => {
    expect(parseProfiles('nope')).toEqual([])
    expect(
      parseProfiles([
        null,
        7,
        { id: '1', server: 'fs01' },
        { id: '1', server: 'fs02' },
        { id: '', server: 'fs03' },
        { id: '2', server: 'C:\\x' },
        { id: '3', server: 'nas', name: 5 },
      ]),
    ).toEqual([
      { id: '1', name: 'fs01', server: 'fs01', user: '', notes: '' },
      { id: '3', name: 'nas', server: 'nas', user: '', notes: '' },
    ])
    localStorage.setItem(SERVERS_KEY, '{not json')
    expect(loadProfiles()).toEqual([])
  })
})

describe('share listing', () => {
  const answer = (server: string): ServerShares => ({
    server,
    shares: [],
    error: null,
    accessDenied: false,
  })

  it('a slow answer from an earlier server never fills the list', async () => {
    const releases: Record<string, () => void> = {}
    const lister = createShareLister(
      (server) =>
        new Promise<ServerShares>((resolve) => {
          releases[server] = () => resolve(answer(server))
        }),
    )
    const first = lister('slow')
    const second = lister('fast')
    releases.fast?.()
    releases.slow?.()
    expect(await second).toEqual(answer('fast'))
    expect(await first).toBeNull()
  })

  it('answers when nothing newer was asked', async () => {
    const lister = createShareLister((server) => Promise.resolve(answer(server)))
    expect(await lister('fs01')).toEqual(answer('fs01'))
  })
})

describe('sign-in errors', () => {
  it('recognises the errors a share sign-in can fix', async () => {
    const { needsSignIn } = await import('./servers')
    expect(needsSignIn('Access is denied. (os error 5)')).toBe(true)
    expect(needsSignIn('Access denied for that account.')).toBe(true)
    expect(needsSignIn('The user name or password is wrong. (os error 1326)')).toBe(true)
    expect(needsSignIn('The system cannot find the path specified. (os error 3)')).toBe(false)
    expect(needsSignIn('')).toBe(false)
  })
})
