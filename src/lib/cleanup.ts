import { Channel, invoke, isTauri } from '@tauri-apps/api/core'
import { reactive, ref } from 'vue'
import { errorMessage, type EntryKind, type ScanProgress } from './compare'
import { currentJob } from './job'

export interface Drive {
  /** "C:\". */
  path: string
  label: string
  total: number
  free: number
  removable: boolean
}

export interface CleanupRow {
  id: number
  name: string
  kind: EntryKind
  /** Size on disk. */
  size: number
  /** Logical size, what Explorer shows as "Size". */
  logical: number
  /** Another name for the same data is in this scan; only one name carries the disk space. */
  hardLinked: boolean
  files: number
  dirs: number
  cloudFiles: number
  /** Online-only bytes below this row, left out of `size`. */
  cloudBytes: number
  errors: number
  error: string | null
  modifiedMs: number | null
  hasChildren: boolean
}

export interface FileRow {
  id: number
  name: string
  folder: string
  size: number
  logical: number
  hardLinked: boolean
  modifiedMs: number | null
}

export interface TypeRow {
  /** Lowercase, no dot; empty when the files have no extension. */
  extension: string
  size: number
  logical: number
  files: number
}

/** Which size the lists, treemap and totals show. */
export type SizeMode = 'disk' | 'logical'

const SIZE_MODE_KEY = 'analyzer-cleanup-size-mode'

function loadSizeMode(): SizeMode {
  try {
    return localStorage.getItem(SIZE_MODE_KEY) === 'logical' ? 'logical' : 'disk'
  } catch {
    return 'disk'
  }
}

export const sizeMode = ref<SizeMode>(loadSizeMode())

export function setSizeMode(mode: SizeMode): void {
  sizeMode.value = mode
  try {
    localStorage.setItem(SIZE_MODE_KEY, mode)
  } catch {
    // The choice just won't be remembered.
  }
}

/** The size to show for an item in the chosen mode. */
export function shownBytes(item: { size: number; logical: number }, mode: SizeMode): number {
  return mode === 'logical' ? item.logical : item.size
}

/** Rows with `size` swapped for the chosen mode, largest first. */
export function rowsFor(rows: CleanupRow[], mode: SizeMode): CleanupRow[] {
  if (mode === 'disk') {
    return rows
  }
  return rows
    .map((row) => ({ ...row, size: row.logical }))
    .sort((a, b) => b.size - a.size || a.name.localeCompare(b.name))
}

export interface Overview {
  root: CleanupRow
  largestFiles: FileRow[]
  /** Every extension, largest first. */
  fileTypes: TypeRow[]
}

interface ScanResult {
  path: string
  overview: Overview
  rows: CleanupRow[]
  elapsedMs: number
  reportId: string | null
}

/**
 * The scan and where the user is in it. Kept outside the page so a long drive scan survives
 * moving to another page and back.
 */
export const cleanupJob = reactive({
  running: false,
  /** The folder being scanned, or the one the result is for. */
  path: '',
  progress: null as ScanProgress | null,
  startedAt: 0,
  elapsedMs: 0,
  finishedAt: null as Date | null,
  overview: null as Overview | null,
  /** Folders from the scanned root down to the one shown; the root has id 0. */
  trail: [] as CleanupRow[],
  /** What is directly inside the last folder of the trail. */
  rows: [] as CleanupRow[],
  error: '',
  notice: '',
  /** The scan's saved report on the Reports page. */
  reportId: null as string | null,
})

/** This PC's drives, for the drive tiles on Home and Disk Cleanup. */
export const drives = ref<Drive[]>([])

export async function refreshDrives(): Promise<void> {
  if (!isTauri()) {
    return
  }
  try {
    drives.value = await invoke<Drive[]>('cleanup_drives')
  } catch {
    // No tiles; a path can still be typed.
  }
}

/** Scans a drive or folder; progress arrives about every 100 ms. */
export async function runScan(path: string): Promise<void> {
  if (cleanupJob.running) {
    return
  }
  Object.assign(cleanupJob, {
    running: true,
    path,
    progress: null,
    startedAt: Date.now(),
    overview: null,
    trail: [],
    rows: [],
    error: '',
    notice: '',
    reportId: null,
  })
  const onProgress = new Channel<ScanProgress>()
  onProgress.onmessage = (progress) => {
    cleanupJob.progress = progress
  }
  try {
    const result = await invoke<ScanResult>('cleanup_scan', {
      path,
      job: currentJob(),
      onProgress,
    })
    Object.assign(cleanupJob, {
      path: result.path,
      overview: result.overview,
      trail: [result.overview.root],
      rows: result.rows,
      elapsedMs: result.elapsedMs,
      reportId: result.reportId,
      finishedAt: new Date(),
    })
  } catch (err) {
    if (err === 'cancelled') {
      cleanupJob.notice = 'Scan cancelled.'
    } else {
      cleanupJob.error = errorMessage(err)
    }
  } finally {
    cleanupJob.running = false
  }
}

export function cancelScan(): Promise<void> {
  return invoke('cleanup_cancel')
}

/** Shows what is inside `row`, a folder in the shown one. */
export async function openFolder(row: CleanupRow): Promise<void> {
  cleanupJob.rows = await invoke<CleanupRow[]>('cleanup_children', { id: row.id })
  cleanupJob.trail = [...cleanupJob.trail, row]
}

/** Goes back up to the folder at `index` in the trail. */
export async function openTrail(index: number): Promise<void> {
  const folder = cleanupJob.trail[index]
  if (!folder) {
    return
  }
  cleanupJob.rows = await invoke<CleanupRow[]>('cleanup_children', { id: folder.id })
  cleanupJob.trail = cleanupJob.trail.slice(0, index + 1)
}

/**
 * Recycles (or deletes for good) one item, then refreshes the totals, the lists and the shown
 * folder. Only the trail's names and ids are used, so its other sizes may be stale.
 */
export async function removeItem(id: number, permanent: boolean): Promise<void> {
  const overview = await invoke<Overview>('cleanup_delete', { id, permanent })
  cleanupJob.overview = overview
  cleanupJob.trail = [overview.root, ...cleanupJob.trail.slice(1)]
  const shown = cleanupJob.trail.at(-1)
  if (shown) {
    cleanupJob.rows = await invoke<CleanupRow[]>('cleanup_children', { id: shown.id })
  }
}

export function itemPath(id: number): Promise<string> {
  return invoke<string>('cleanup_path', { id })
}

/** Opens Explorer with the item selected. */
export function revealItem(id: number): Promise<void> {
  return invoke('cleanup_reveal', { id })
}

/** "C:", "c:\", "C:/" all match the drive "C:\". */
export function driveFor(path: string, drives: Drive[]): Drive | undefined {
  const key = (value: string): string =>
    value
      .trim()
      .replace(/[\\/]+$/, '')
      .toLowerCase()
  return drives.find((drive) => key(drive.path) === key(path))
}

/**
 * How far a drive scan has got, from the bytes found so far against the drive's used space.
 * Capped at 99 until the scan ends; `null` when the total isn't known.
 */
export function scanPercent(bytes: number, drive: Drive | undefined): number | null {
  const used = drive ? drive.total - drive.free : 0
  if (used <= 0) {
    return null
  }
  return Math.min(99, Math.floor((bytes / used) * 100))
}

export type QuickId =
  | 'userTemp'
  | 'windowsTemp'
  | 'browserCache'
  | 'updateCache'
  | 'recycleBin'
  | 'oldInstallers'
  | 'internetCache'
  | 'crashReports'
  | 'shaderCaches'
  | 'playwright'

/** A quick cleanup card. */
export interface QuickWin {
  id: QuickId
  title: string
  description: string
  needsAdmin: boolean
  /** What it would free now. */
  size: number
  files: number
}

export interface QuickList {
  /** Running as administrator, so the system cleanups can run. */
  elevated: boolean
  items: QuickWin[]
}

export interface CleanedItem {
  title: string
  freed: number
  removedFiles: number
  skippedFiles: number
  error: string | null
}

export interface QuickResult {
  report: { items: CleanedItem[] }
  reportId: string | null
}

/** Measures every quick cleanup; takes a few seconds on a big profile. */
export function loadQuickWins(): Promise<QuickList> {
  return invoke<QuickList>('cleanup_quick_list')
}

/** Runs the chosen quick cleanups and saves a report of what they freed. */
export function runQuickClean(ids: QuickId[]): Promise<QuickResult> {
  return invoke<QuickResult>('cleanup_quick_clean', { ids, job: currentJob() })
}

/** Restarts Deep Server Analyzer as administrator; Windows asks first. */
export function restartAsAdmin(): Promise<void> {
  return invoke('app_restart_admin')
}

/** Which cards can be ticked: something to free, and admin rights when needed. */
export function canClean(item: QuickWin, elevated: boolean): boolean {
  return item.size > 0 && (!item.needsAdmin || elevated)
}

/** Whether Deep Server Analyzer runs as administrator; read once at start. */
export const appAdmin = reactive({ elevated: false })

export async function refreshElevated(): Promise<void> {
  if (!isTauri()) {
    return
  }
  try {
    appAdmin.elevated = await invoke<boolean>('app_elevated')
  } catch {
    // Treated as not elevated: the admin-only buttons stay disabled.
  }
}

export type JunkKind =
  | 'tempFiles'
  | 'crashDumps'
  | 'oldLogs'
  | 'thumbnails'
  | 'installers'
  | 'devCaches'
  | 'emptyFolders'

export interface JunkItem {
  /** Keys the row. For a collapsed row, the first of `ids`. */
  id: number
  /** Every node deleting this row removes: `[id]`, or each `__pycache__` folder of a project. */
  ids: number[]
  name: string
  path: string
  size: number
  files: number
  isFolder: boolean
  modifiedMs: number | null
}

export interface JunkGroup {
  kind: JunkKind
  title: string
  description: string
  /** Every match, including those past the listed items. */
  size: number
  count: number
  items: JunkItem[]
}

export type PreviewStatus = 'go' | 'protected' | 'reparse' | 'gone' | 'inside'

export interface PreviewItem {
  id: number
  name: string
  path: string
  size: number
  files: number
  isFolder: boolean
  status: PreviewStatus
  reason: string | null
}

export interface DeletePreview {
  items: PreviewItem[]
  goCount: number
  goSize: number
}

export type Outcome = 'done' | 'inUse' | 'accessDenied' | 'notFound' | 'skipped' | 'failed'

export interface ItemResult {
  id: number
  path: string
  size: number
  outcome: Outcome
  message: string | null
}

export interface DeleteManyResult {
  results: ItemResult[]
  overview: Overview
  /** Set when the deletes happened but the deletion log could not be written. */
  logError?: string | null
}

/** Suggested junk on the scanned tree; nothing is ticked for the user. */
export function loadJunk(): Promise<JunkGroup[]> {
  return invoke<JunkGroup[]>('cleanup_junk')
}

/** Dry run: what a delete of these items would do. Removes nothing. */
export function previewDelete(ids: number[]): Promise<DeletePreview> {
  return invoke<DeletePreview>('cleanup_delete_preview', { ids })
}

/**
 * Deletes the items one by one, then refreshes the totals and the shown folder. Resolves with
 * a result for each item that went ahead.
 */
export async function removeMany(ids: number[], permanent: boolean): Promise<ItemResult[]> {
  const done = await invoke<DeleteManyResult>('cleanup_delete_many', { ids, permanent })
  cleanupJob.overview = done.overview
  if (done.logError) {
    cleanupJob.notice = done.logError
  }
  cleanupJob.trail = [done.overview.root, ...cleanupJob.trail.slice(1)]
  const shown = cleanupJob.trail.at(-1)
  if (shown) {
    cleanupJob.rows = await invoke<CleanupRow[]>('cleanup_children', { id: shown.id })
  }
  return done.results
}

/** Several items or a permanent delete need administrator; one item to the Recycle Bin does not. */
export function needsAdmin(count: number, permanent: boolean): boolean {
  return permanent || count > 1
}

export function canDelete(count: number, permanent: boolean, elevated: boolean): boolean {
  return count > 0 && (elevated || !needsAdmin(count, permanent))
}

/** The sentence under the dialog's totals. */
export function deleteSentence(count: number, permanent: boolean): string {
  const noun = count === 1 ? 'item' : 'items'
  return permanent
    ? `${String(count)} ${noun} will be deleted permanently. This cannot be undone.`
    : `${String(count)} ${noun} will be moved to the Recycle Bin.`
}

const OUTCOME_LABELS: Record<Outcome, string> = {
  done: 'Deleted',
  inUse: 'In use',
  accessDenied: 'Access denied',
  notFound: 'Already gone',
  skipped: 'Skipped',
  failed: 'Failed',
}

export function outcomeLabel(outcome: Outcome): string {
  return OUTCOME_LABELS[outcome]
}

/** Counts and freed space for the result list. */
export function summarize(results: ItemResult[]): { done: number; failed: number; freed: number } {
  const done = results.filter((result) => result.outcome === 'done')
  return {
    done: done.length,
    failed: results.length - done.length,
    freed: done.reduce((sum, result) => sum + result.size, 0),
  }
}
