import { invoke } from '@tauri-apps/api/core'
import { ref } from 'vue'

/**
 * A saved server. There is no secret field on purpose: the app never stores a password. A
 * profile's user name only pre-fills the sign-in dialog.
 */
export interface ServerProfile {
  id: string
  /** Display name. */
  name: string
  /** The server name, such as `fs01` (no backslashes). */
  server: string
  /** Optional account for the sign-in dialog, such as `CORP\sam`. */
  user: string
  notes: string
}

export const SERVERS_KEY = 'analyzer-servers'

/** `fs01` from `fs01`, `\\fs01` or `\\fs01\Finance\x`; empty when there is no usable name. */
export function serverName(input: string): string {
  const name = input.trim().replace(/\//g, '\\').replace(/^\\+/, '').split('\\')[0]?.trim() ?? ''
  return name === '?' || name === '.' || /[\s:*?"<>|]/.test(name) ? '' : name
}

/** Keeps only the known text fields of each saved entry, so nothing else is ever carried along. */
export function parseProfiles(raw: unknown): ServerProfile[] {
  if (!Array.isArray(raw)) {
    return []
  }
  const seen = new Set<string>()
  const list: ServerProfile[] = []
  for (const item of raw) {
    if (typeof item !== 'object' || item === null) {
      continue
    }
    const entry = item as Record<string, unknown>
    const text = (value: unknown): string => (typeof value === 'string' ? value : '')
    const server = serverName(text(entry.server))
    const id = text(entry.id)
    if (server === '' || id === '' || seen.has(id)) {
      continue
    }
    seen.add(id)
    list.push({
      id,
      name: text(entry.name).trim() || server,
      server,
      user: text(entry.user).trim(),
      notes: text(entry.notes),
    })
  }
  return list
}

/** What is written to storage: the known fields and nothing else (no password, ever). */
export function serializeProfiles(list: ServerProfile[]): string {
  return JSON.stringify(
    list.map(({ id, name, server, user, notes }) => ({ id, name, server, user, notes })),
  )
}

export function loadProfiles(): ServerProfile[] {
  try {
    return parseProfiles(JSON.parse(localStorage.getItem(SERVERS_KEY) ?? '[]'))
  } catch {
    // Unreadable storage: no profiles.
    return []
  }
}

function saveProfiles(list: ServerProfile[]): void {
  try {
    localStorage.setItem(SERVERS_KEY, serializeProfiles(list))
  } catch {
    // Storage full or blocked: the profiles just aren't remembered.
  }
}

/** The saved profiles, in the order the user added them. */
export const profiles = ref<ServerProfile[]>(loadProfiles())

function newId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}

export interface ProfileInput {
  name: string
  server: string
  user: string
  notes: string
}

/** Adds a profile; returns it, or null when the server name is not usable. */
export function addProfile(input: ProfileInput): ServerProfile | null {
  const server = serverName(input.server)
  if (server === '') {
    return null
  }
  const profile: ServerProfile = {
    id: newId(),
    name: input.name.trim() || server,
    server,
    user: input.user.trim(),
    notes: input.notes,
  }
  profiles.value = [...profiles.value, profile]
  saveProfiles(profiles.value)
  return profile
}

/** Changes a profile; false when it is gone or the server name is not usable. */
export function updateProfile(id: string, input: ProfileInput): boolean {
  const server = serverName(input.server)
  if (server === '' || !profiles.value.some((profile) => profile.id === id)) {
    return false
  }
  profiles.value = profiles.value.map((profile) =>
    profile.id === id
      ? {
          id,
          name: input.name.trim() || server,
          server,
          user: input.user.trim(),
          notes: input.notes,
        }
      : profile,
  )
  saveProfiles(profiles.value)
  return true
}

export function removeProfile(id: string): void {
  profiles.value = profiles.value.filter((profile) => profile.id !== id)
  saveProfiles(profiles.value)
}

/** `\\server\share`, the path a folder field takes. */
export function sharePath(server: string, share: string): string {
  return `\\\\${serverName(server)}\\${share}`
}

/** True for an error message that a sign-in to the share could fix (access denied, bad logon). */
export function needsSignIn(message: string): boolean {
  return /access is denied|os error (5|1326|1219)\b|logon failure|access denied/i.test(message)
}

/** A share every Windows server has, to sign in to before listing: `\\server\IPC$`. */
export function signInShare(server: string): string {
  return sharePath(server, 'IPC$')
}

export interface ShareEntry {
  name: string
  comment: string
  /** An administrative or special share (`C$`), listed after the rest. */
  hidden: boolean
}

/** The answer of `servers_list_shares`. A failure is data: `error` says why. */
export interface ServerShares {
  server: string
  shares: ShareEntry[]
  error: string | null
  accessDenied: boolean
}

export function listShares(server: string): Promise<ServerShares> {
  return invoke<ServerShares>('servers_list_shares', { server })
}

/**
 * Runs share listings so that only the newest answer counts: a slow answer from an earlier
 * server resolves to null instead of filling the list for the server asked later.
 */
export function createShareLister(
  list: (server: string) => Promise<ServerShares> = listShares,
): (server: string) => Promise<ServerShares | null> {
  let seq = 0
  return async (server) => {
    const mine = ++seq
    const answer = await list(server)
    return mine === seq ? answer : null
  }
}
