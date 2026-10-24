import { Channel, invoke } from '@tauri-apps/api/core'
import { reactive } from 'vue'
import { trackJob } from './busy'
import { formatBytes, formatDateTime, plural } from './format'
import { currentJob, type JobInfo } from './job'

/** Copy: Deep Server Analyzer copies. Watch: another program copies and Deep Server Analyzer checks what arrives. */
export type RecordMode = 'copy' | 'watch'
export type VerifyLevel = 'sizeAndTime' | 'hash'
/** What a copy keeps besides the bytes. */
/** What a copy does when a different file is already at the destination. */
export type ConflictChoice = 'skip' | 'overwriteIfNewer' | 'overwrite' | 'keepBoth'

export const conflictChoices: { value: ConflictChoice; label: string }[] = [
  { value: 'skip', label: 'Leave it and list the file as not copied' },
  { value: 'overwriteIfNewer', label: 'Replace it only if the source is newer' },
  { value: 'overwrite', label: 'Replace it' },
  { value: 'keepBoth', label: 'Keep both (save the source as "name (2).ext")' },
]

export interface PreserveOptions {
  timestamps: boolean
  attributes: boolean
  acl: boolean
  owner: boolean
  audit: boolean
  streams: boolean
}
export const defaultPreserve: PreserveOptions = {
  timestamps: true,
  attributes: true,
  acl: true,
  owner: true,
  audit: false,
  streams: true,
}
export type RunState = 'prepared' | 'running' | 'watching' | 'completed' | 'cancelled' | 'failed'
export type Phase = 'scanning' | 'copying' | 'watching' | 'finishing' | 'done'

export interface RecordProgress {
  runId: string
  phase: Phase
  filesDone: number
  filesTotal: number
  bytesDone: number
  bytesTotal: number
  copied: number
  skipped: number
  notCopied: number
  current: string | null
}

export interface RunTotals {
  plannedFiles: number
  plannedBytes: number
  folders: number
  /** Links and junctions, which are not followed. */
  excluded: number
  copied: number
  copiedBytes: number
  skippedIdentical: number
  notCopied: number
  notCopiedBytes: number
}

export interface RunSummary {
  id: string
  settings: {
    source: string
    destination: string
    mode: RecordMode
    verify: VerifyLevel
    ignoreJunk: boolean
    downloadCloud: boolean
  }
  state: RunState
  createdAtMs: number
  startedAtMs: number | null
  finishedAtMs: number | null
  machine: string
  user: string
  totals: RunTotals
  error: string | null
}

export type FailureReason =
  | 'accessDenied'
  | 'fileLocked'
  | 'diskFull'
  | 'pathTooLong'
  | 'invalidName'
  | 'nameCollision'
  | 'networkLost'
  | 'deviceRemoved'
  | 'mediaError'
  | 'sourceVanished'
  | 'targetExistsDifferent'
  | 'verifyMismatch'
  | 'missing'
  | 'incomplete'
  | 'cloudOnly'
  | 'cancelled'
  | 'unknown'

export interface ItemResult {
  /** Relative to the source and destination, with `/` separators. */
  relativePath: string
  kind: 'file' | 'folder'
  size: number
  status: 'copied' | 'arrived' | 'skippedIdentical' | 'notCopied'
  reason?: FailureReason
  side?: 'source' | 'destination'
  osCode?: number
  message?: string
  attempts: number
  /** Watch mode: the reason is a best guess, since Deep Server Analyzer did not do the copy. */
  inferred: boolean
  recoverable: boolean
  atMs: number
}

export interface PreflightReport {
  files: number
  folders: number
  bytes: number
  alreadyThere: number
  bytesNeeded: number
  freeBytes: number | null
  volume: string | null
  scanErrors: number
  blocked: number
  /** Pre-flight issues, including warnings such as long paths. */
  issueCount: number
}

export interface RunDetails {
  summary: RunSummary
  preflight: PreflightReport | null
  /** The first 10,000 not-copied rows; `summary.totals` counts all of them. */
  notCopied: ItemResult[]
  /** Client, ticket and technician typed when it started. */
  job: JobInfo
  folder: string
}

export interface RecoveryResult {
  folder: string
  listFile: string
  copied: number
  skipped: { relativePath: string; message: string }[]
}

/**
 * The running record, kept outside the page so leaving it and coming back still shows the live
 * panel with Finish and Cancel. The backend runs one record at a time.
 */
export const recordJob = reactive({
  running: false,
  mode: 'watch' as RecordMode,
  progress: null as RecordProgress | null,
  /** When copying or watching began (after the source was listed), for the speed. */
  copyStartedAt: 0,
  finishing: false,
})

/** The error text the backend returns when a record is cancelled before it got going. */
export function isCancelled(error: unknown): boolean {
  return error === 'Cancelled'
}

function channel(onProgress: (progress: RecordProgress) => void): Channel<RecordProgress> {
  const onEvent = new Channel<RecordProgress>()
  onEvent.onmessage = onProgress
  return onEvent
}

/** Lists the source, then copies or watches. Resolves when the record has ended. */
export function startRecord(
  source: string,
  destination: string,
  mode: RecordMode,
  verify: VerifyLevel,
  options: {
    ignoreJunk: boolean
    downloadCloud: boolean
    preserve: PreserveOptions
    conflict?: ConflictChoice
  },
  onProgress: (progress: RecordProgress) => void,
): Promise<RunDetails> {
  return trackJob(() =>
    invoke<RunDetails>('record_start', {
      source,
      destination,
      mode,
      verify,
      options: { ...options, job: currentJob() },
      onEvent: channel(onProgress),
    }),
  )
}

/** Compare's "Copy missing files": copies what the last compare found missing, as a record. */
export function copyMissingRecord(
  verify: VerifyLevel,
  onProgress: (progress: RecordProgress) => void,
  preserve: PreserveOptions = defaultPreserve,
): Promise<RunDetails> {
  return trackJob(() =>
    invoke<RunDetails>('record_copy_missing', {
      verify,
      job: currentJob(),
      preserve,
      onEvent: channel(onProgress),
    }),
  )
}

/** Copies the not-copied files again: `paths` and/or one `reason`, or all of them when neither. */
export function retryRecord(
  runId: string,
  paths: string[],
  onProgress: (progress: RecordProgress) => void,
  reason: FailureReason | null = null,
): Promise<RunDetails> {
  return trackJob(() =>
    invoke<RunDetails>('record_retry', { runId, paths, reason, onEvent: channel(onProgress) }),
  )
}

/** Watch mode: stop watching and check every file that has not arrived. */
export function finishRecord(): Promise<boolean> {
  return trackJob(() => invoke<boolean>('record_finish'))
}

export function cancelRecord(): Promise<boolean> {
  return invoke<boolean>('record_cancel')
}

/** Copies every not-copied file to `folder` (default: beside the destination) with a list. */
export function recoverRecord(runId: string, folder: string | null): Promise<RecoveryResult> {
  return invoke<RecoveryResult>('record_recover', { runId, folder })
}

export function loadRecord(runId: string): Promise<RunDetails> {
  return invoke<RunDetails>('record_load', { runId })
}

/** Stored records, newest first. */
export function listRecords(): Promise<RunSummary[]> {
  return invoke<RunSummary[]>('record_list')
}

/** The `\\server\share` part of a network path, or null for a local path. */
export function shareOf(path: string): string | null {
  const match = /^[\\/]{2}(?:\?[\\/]UNC[\\/])?([^\\/?.][^\\/]*)[\\/]+([^\\/]+)/.exec(path.trim())
  return match ? `\\\\${match[1]}\\${match[2]}` : null
}

/** Signs in to the share a network path is on; resolves with the share. */
export function connectShare(path: string, username: string, password: string): Promise<string> {
  return invoke<string>('share_connect', { path, username, password })
}

/** Which folder of a run `revealRecord` opens. */
export type RevealSide = 'source' | 'destination' | 'recovery'

/** Opens Explorer at a file (by relative path) or the recovery folder of a run. */
export function revealRecord(runId: string, relativePath: string, side: RevealSide): Promise<void> {
  return invoke('record_reveal', { runId, relativePath, side })
}

const reasonTitles: Record<FailureReason, string> = {
  accessDenied: 'Access denied',
  fileLocked: 'In use by another program',
  diskFull: 'Disk full',
  pathTooLong: 'Path too long',
  invalidName: 'Name not allowed on Windows',
  nameCollision: 'Name differs only by letter case',
  networkLost: 'Network connection lost',
  deviceRemoved: 'Drive removed or not ready',
  mediaError: 'Read or write error on the disk',
  sourceVanished: 'Source file no longer exists',
  targetExistsDifferent: 'A different file is already at the destination',
  verifyMismatch: 'Copy does not match the source',
  missing: 'Never arrived at the destination',
  incomplete: 'Still incomplete at the destination',
  cloudOnly: 'Online-only (not downloaded)',
  cancelled: 'Cancelled',
  unknown: 'Other error',
}

export function reasonTitle(reason: FailureReason | undefined): string {
  return reason ? reasonTitles[reason] : reasonTitles.unknown
}

/** Not-copied rows per reason, most common first. */
export function countByReason(items: ItemResult[]): { reason: FailureReason; count: number }[] {
  const counts = new Map<FailureReason, number>()
  for (const item of items) {
    const reason = item.reason ?? 'unknown'
    counts.set(reason, (counts.get(reason) ?? 0) + 1)
  }
  return [...counts]
    .map(([reason, count]) => ({ reason, count }))
    .sort((a, b) => b.count - a.count || reasonTitle(a.reason).localeCompare(reasonTitle(b.reason)))
}

/** Average speed in bytes per second since the copy started; 0 until there is something. */
export function bytesPerSecond(bytesDone: number, elapsedMs: number): number {
  return elapsedMs >= 1000 && bytesDone > 0 ? (bytesDone * 1000) / elapsedMs : 0
}

/** Time left at the average speed, or `null` when it can't be told yet (the first 3 s). */
export function timeLeftMs(
  bytesDone: number,
  bytesTotal: number,
  elapsedMs: number,
): number | null {
  const speed = bytesPerSecond(bytesDone, elapsedMs)
  if (elapsedMs < 3000 || speed === 0 || bytesTotal <= bytesDone) {
    return null
  }
  return ((bytesTotal - bytesDone) / speed) * 1000
}

/** "About 3 min left", "About 40 s left", "Less than 10 s left". */
export function formatTimeLeft(ms: number): string {
  if (ms < 10_000) {
    return 'Less than 10 s left'
  }
  if (ms < 60_000) {
    return `About ${String(Math.round(ms / 10_000) * 10)} s left`
  }
  if (ms < 3_600_000) {
    return `About ${plural(Math.round(ms / 60_000), 'min', 'min')} left`
  }
  const hours = Math.floor(ms / 3_600_000)
  const minutes = Math.round((ms % 3_600_000) / 60_000)
  return `About ${String(hours)} h ${String(minutes).padStart(2, '0')} min left`
}

/** Plain-text list of the not-copied files, ready to paste into an email or ticket. */
export function formatNotCopiedList(details: RunDetails): string {
  const { summary } = details
  const header = [
    `Not copied: ${plural(summary.totals.notCopied, 'item')} (${formatBytes(summary.totals.notCopiedBytes)})`,
    `Source: ${summary.settings.source}`,
    `Destination: ${summary.settings.destination}`,
    `Recorded: ${formatDateTime(summary.createdAtMs)}`,
  ]
  const rows = details.notCopied.map(
    (item) => `${item.relativePath.replaceAll('/', '\\')}\t${reasonTitle(item.reason)}`,
  )
  return [...header, '', ...rows].join('\r\n')
}
