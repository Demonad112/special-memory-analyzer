import { Channel, invoke, isTauri } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { ref, watch } from 'vue'
import { trackJob } from './busy'
import { formatBytes, plural } from './format'
import { currentJob } from './job'

const ignoreJunkKey = 'analyzer-ignore-junk'

/**
 * "Ignore system and temp files" (Thumbs.db, desktop.ini, ~$ Office lock files, *.tmp, ...),
 * shared by Compare and Record. On unless turned off.
 */
export const ignoreJunk = ref(readIgnoreJunk())

function readIgnoreJunk(): boolean {
  try {
    return localStorage.getItem(ignoreJunkKey) !== 'false'
  } catch {
    return true
  }
}

watch(ignoreJunk, (value) => {
  try {
    localStorage.setItem(ignoreJunkKey, String(value))
  } catch {
    // Not remembered; the toggle still applies for this session.
  }
})

export type EntryKind = 'file' | 'dir' | 'link'
export type DiffStatus = 'same' | 'different' | 'onlyLeft' | 'onlyRight' | 'kindMismatch'

export interface DiffSide {
  size: number
  files: number
  dirs: number
}

/** One row of a compare. Left is the source, right the destination. */
export interface DiffRow {
  id: number
  name: string
  kind: EntryKind
  left: DiffSide | null
  right: DiffSide | null
  status: DiffStatus
  /** Source files at or below this row that the destination lacks. */
  missing: number
  missingBytes: number
  /** Destination files at or below this row that the source lacks. */
  extra: number
  /** Files at or below this row whose size differs. */
  different: number
  cloud: boolean
  error: string | null
  hasChildren: boolean
}

export interface DiffSummary {
  left: DiffSide
  right: DiffSide
  missing: number
  missingBytes: number
  extra: number
  different: number
  leftErrors: number
  rightErrors: number
  leftCloud: number
  rightCloud: number
}

export interface ScanProgress {
  files: number
  dirs: number
  bytes: number
  current: string
}

export interface CompareProgress {
  left: ScanProgress
  right: ScanProgress
  leftDone: boolean
  rightDone: boolean
}

export type Side = 'left' | 'right'

export interface CompareResult {
  summary: DiffSummary
  /** Rows directly below the two compared folders. */
  rows: DiffRow[]
  elapsedMs: number
  /** The saved report on the Reports page; null when it could not be saved. */
  reportId: string | null
}

type CompareEvent = { kind: 'progress' } & CompareProgress

/** The error text the backend returns for a cancelled compare. */
export const CANCELLED = 'cancelled'

export function isCancelled(error: unknown): boolean {
  return error === CANCELLED
}

export function errorMessage(error: unknown): string {
  if (typeof error === 'string') {
    return error
  }
  return error instanceof Error ? error.message : 'Something went wrong.'
}

/** Scans both folders and compares them. Progress arrives about every 100 ms. */
export async function startCompare(
  left: string,
  right: string,
  ignoreJunk: boolean,
  onProgress: (progress: CompareProgress) => void,
): Promise<CompareResult> {
  const onEvent = new Channel<CompareEvent>()
  onEvent.onmessage = (event) => {
    onProgress({
      left: event.left,
      right: event.right,
      leftDone: event.leftDone,
      rightDone: event.rightDone,
    })
  }
  return trackJob(() =>
    invoke<CompareResult>('compare_start', {
      left,
      right,
      ignoreJunk,
      job: currentJob(),
      onEvent,
    }),
  )
}

/** Rows below one folder of the last compare. */
export function loadChildren(id: number): Promise<DiffRow[]> {
  return invoke<DiffRow[]>('compare_children', { id })
}

export function cancelCompare(): Promise<void> {
  return invoke('compare_cancel')
}

/** Full path of a row on one side of the last compare. */
export function rowPath(id: number, side: Side): Promise<string> {
  return invoke<string>('compare_path', { id, side })
}

/** Opens Explorer with the row's file or folder selected. */
export function revealRow(id: number, side: Side): Promise<void> {
  return invoke('compare_reveal', { id, side })
}

export interface MissingList {
  source: string
  destination: string
  /** Relative to the source; empty folders end with a separator. */
  paths: string[]
}

export function loadMissing(): Promise<MissingList> {
  return invoke<MissingList>('compare_missing')
}

/** Plain-text list of missing files, ready to paste into an email or ticket. */
export function formatMissingList(list: MissingList, summary: DiffSummary, when: Date): string {
  const header = [
    `Missing at destination: ${plural(summary.missing, 'file')} (${formatBytes(summary.missingBytes)})`,
    `Source: ${list.source}`,
    `Destination: ${list.destination}`,
    `Compared: ${when.toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })}`,
  ]
  return [...header, '', ...list.paths].join('\r\n')
}

/** Trims spaces and the quotes Explorer's "Copy as path" adds. */
export function cleanPath(input: string): string {
  const trimmed = input.trim()
  const unquoted =
    trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')
      ? trimmed.slice(1, -1)
      : trimmed
  return unquoted.trim()
}

/** Native folder picker. `null` when cancelled or outside the desktop app. */
export async function pickFolder(title: string, defaultPath?: string): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  const picked = await open({
    directory: true,
    multiple: false,
    title,
    defaultPath: defaultPath || undefined,
  })
  return typeof picked === 'string' && picked !== '' ? picked : null
}

export interface ComparePaths {
  left: string
  right: string
}

const pathsKey = 'analyzer-compare-paths'

export function loadPaths(): ComparePaths {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(pathsKey) ?? 'null')
    if (saved && typeof saved === 'object') {
      const { left, right } = saved as Record<string, unknown>
      return {
        left: typeof left === 'string' ? left : '',
        right: typeof right === 'string' ? right : '',
      }
    }
  } catch {
    // Unreadable storage: start empty.
  }
  return { left: '', right: '' }
}

export function savePaths(paths: ComparePaths): void {
  try {
    localStorage.setItem(pathsKey, JSON.stringify({ left: paths.left, right: paths.right }))
  } catch {
    // Storage full or blocked: the paths just aren't remembered.
  }
}

const recentKey = 'analyzer-compare-recent'
const recentLimit = 5

/** Last compared folder pairs, newest first. */
export function loadRecent(): ComparePaths[] {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(recentKey) ?? '[]')
    if (!Array.isArray(saved)) {
      return []
    }
    return saved
      .filter(
        (item): item is ComparePaths =>
          typeof item === 'object' &&
          item !== null &&
          typeof (item as Record<string, unknown>).left === 'string' &&
          typeof (item as Record<string, unknown>).right === 'string',
      )
      .slice(0, recentLimit)
  } catch {
    return []
  }
}

/** The missing count of the last finished compare, to show the change on a re-check. */
let lastMissing: { left: string; right: string; missing: number } | null = null

/** What changed since the last compare of the same pair ("612 → 4 still missing"), or null. */
export function missingDelta(pair: ComparePaths, missing: number): string | null {
  const before =
    lastMissing &&
    lastMissing.left.toLowerCase() === pair.left.toLowerCase() &&
    lastMissing.right.toLowerCase() === pair.right.toLowerCase()
      ? lastMissing.missing
      : null
  lastMissing = { left: pair.left, right: pair.right, missing }
  if (before === null || before === 0) {
    return null
  }
  if (missing === 0) {
    return `All ${String(before)} missing files are now at the destination`
  }
  if (missing === before) {
    return `${String(missing)} still missing, no change since the last compare`
  }
  return `${String(before)} → ${String(missing)} still missing`
}

/** Forgets the last compare (tests). */
export function resetMissingDelta(): void {
  lastMissing = null
}

export function rememberRecent(pair: ComparePaths): ComparePaths[] {
  const same = (item: ComparePaths): boolean =>
    item.left.toLowerCase() === pair.left.toLowerCase() &&
    item.right.toLowerCase() === pair.right.toLowerCase()
  const recent = [
    { left: pair.left, right: pair.right },
    ...loadRecent().filter((item) => !same(item)),
  ].slice(0, recentLimit)
  try {
    localStorage.setItem(recentKey, JSON.stringify(recent))
  } catch {
    // Not remembered; the list still shows for this session.
  }
  return recent
}
