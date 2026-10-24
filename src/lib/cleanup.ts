import { Channel, invoke, isTauri } from '@tauri-apps/api/core'
import { reactive, ref } from 'vue'
import { trackJob } from './busy'
import { type EntryKind, type ScanProgress } from './compare'
import { errorMessage } from './errors'
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
  /** Set when the saved report could not be updated; the delete still went ahead. */
  warning?: string | null
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
export function runScan(path: string): Promise<void> {
  return trackJob(() => scanFolder(path))
}

async function scanFolder(path: string): Promise<void> {
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
  navSeq++
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
    if (result.overview.warning) {
      cleanupJob.notice = result.overview.warning
    }
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

/** Escape cancels a running scan unless it is meant for a menu or dialog that is open. */
export function escapeCancelsScan(key: string, running: boolean, covered: boolean): boolean {
  return key === 'Escape' && running && !covered
}

/** The tab an arrow, Home or End key moves to in a row of `count` tabs, or null for other keys. */
export function nextTabIndex(key: string, index: number, count: number): number | null {
  switch (key) {
    case 'ArrowRight':
      return (index + 1) % count
    case 'ArrowLeft':
      return (index - 1 + count) % count
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}

export function cancelScan(): Promise<void> {
  return invoke('cleanup_cancel')
}

/** Counts navigations so a slow earlier one cannot overwrite a newer one. */
let navSeq = 0

/** Shows what is inside `row`, a folder in the shown one. */
export async function openFolder(row: CleanupRow): Promise<void> {
  const seq = ++navSeq
  const rows = await invoke<CleanupRow[]>('cleanup_children', { id: row.id })
  if (seq !== navSeq) {
    return
  }
  cleanupJob.rows = rows
  cleanupJob.trail = [...cleanupJob.trail, row]
}

/** Goes back up to the folder at `index` in the trail. */
export async function openTrail(index: number): Promise<void> {
  const folder = cleanupJob.trail[index]
  if (!folder) {
    return
  }
  const seq = ++navSeq
  const rows = await invoke<CleanupRow[]>('cleanup_children', { id: folder.id })
  if (seq !== navSeq) {
    return
  }
  cleanupJob.rows = rows
  cleanupJob.trail = cleanupJob.trail.slice(0, index + 1)
}

/**
 * Recycles (or deletes for good) one item, then refreshes the totals, the lists and the shown
 * folder. Only the trail's names and ids are used, so its other sizes may be stale.
 */
export async function removeItem(id: number, permanent: boolean): Promise<void> {
  const overview = await trackJob(() => invoke<Overview>('cleanup_delete', { id, permanent }))
  cleanupJob.overview = overview
  if (overview.warning) {
    cleanupJob.notice = overview.warning
  }
  cleanupJob.trail = [overview.root, ...cleanupJob.trail.slice(1)]
  const shown = cleanupJob.trail.at(-1)
  if (shown) {
    const seq = ++navSeq
    const rows = await invoke<CleanupRow[]>('cleanup_children', { id: shown.id })
    if (seq === navSeq) {
      cleanupJob.rows = rows
    }
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
  return trackJob(() => invoke<QuickResult>('cleanup_quick_clean', { ids, job: currentJob() }))
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
  | 'appCaches'
  | 'browserData'
  | 'unidentified'

/** Which list a finding sits in: `never` rows are reported by size and carry no ids. */
export type Tier = 'safe' | 'review' | 'never'

/** What removing it costs, least to most. */
export type Confidence = 'regenerates' | 'redownloads' | 'user_data'

export interface JunkItem {
  /** Keys the row. For a collapsed row, the first of `ids`. */
  id: number
  /**
   * Every node deleting this row removes: `[id]`, or each folder a collapsed row stands for.
   * Empty for a `never` row: it is reported, not offered.
   */
  ids: number[]
  name: string
  path: string
  /** Space on disk: online-only (cloud) files count as 0. */
  size: number
  files: number
  isFolder: boolean
  modifiedMs: number | null
  /** Online-only bytes below this row, left out of `size`; say so when above 0. */
  cloudBytes: number
  tier: Tier
  confidence: Confidence
  /** Why it is suggested, or what it was recognised as. */
  why: string
  /** How to get it back, when there is something to say. */
  restore: string | null
}

export interface JunkGroup {
  kind: JunkKind
  /** A group holds one tier: a kind with rows in two tiers is two groups. Safe first. */
  tier: Tier
  /** The least forgiving confidence of any match in the group. */
  confidence: Confidence
  title: string
  description: string
  why: string
  /** Every match, including those past the listed items. */
  size: number
  count: number
  /** Online-only bytes below every match, left out of `size`. */
  cloudBytes: number
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

/** Set on a dry run big enough that the delete needs a second confirmation. */
export interface Brake {
  /** Handed back to the delete to show the second confirmation was answered. */
  token: string
  /** The largest items that would go, biggest first. */
  biggest: PreviewItem[]
}

export interface DeletePreview {
  items: PreviewItem[]
  goCount: number
  goSize: number
  brake?: Brake | null
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

/** Tiers in the order the list shows them. */
export const TIERS: Tier[] = ['safe', 'review', 'never']

export const TIER_TITLES: Record<Tier, string> = {
  safe: 'Safe',
  review: 'Review',
  never: 'Never listed',
}

/** A line under the tier's heading; empty when there is nothing to add. */
export const TIER_HINTS: Record<Tier, string> = {
  safe: '',
  review: 'Check each one before removing it.',
  never: 'Shown for information only. Never offered for deletion.',
}

/** Badge text and colour per confidence; colours are `--app-*` tokens via the theme. */
export const CONFIDENCE_BADGES: Record<Confidence, { label: string; tone: string }> = {
  regenerates: { label: 'Regenerates', tone: 'border-success/40 text-success' },
  redownloads: { label: 'Re-downloads', tone: 'border-accent/40 text-accent' },
  user_data: { label: 'Your data', tone: 'border-warning/40 text-warning' },
}

/** The groups in one tier, in the order they came. */
export function groupsIn(groups: JunkGroup[], tier: Tier): JunkGroup[] {
  return groups.filter((group) => group.tier === tier)
}

/** The rows in a tier that can be ticked. A `never` row has no ids, so it never can. */
export function offeredItems(groups: JunkGroup[], tier: Tier): JunkItem[] {
  return groupsIn(groups, tier).flatMap((group) =>
    group.items.filter((item) => item.ids.length > 0),
  )
}

/** Whether every offered row in a tier is ticked; false when the tier offers none. */
export function allTicked(groups: JunkGroup[], tier: Tier, ticked: ReadonlySet<number>): boolean {
  const items = offeredItems(groups, tier)
  return items.length > 0 && items.every((item) => ticked.has(item.id))
}

/** "Select all" for a tier: ticks its offered rows, or clears them when they are all ticked. */
export function toggleTier(
  groups: JunkGroup[],
  tier: Tier,
  ticked: ReadonlySet<number>,
): Set<number> {
  const clear = allTicked(groups, tier, ticked)
  const next = new Set(ticked)
  for (const item of offeredItems(groups, tier)) {
    if (clear) {
      next.delete(item.id)
    } else {
      next.add(item.id)
    }
  }
  return next
}

/** The junk row behind each node id, so the dry run can show why each node is there. */
export function rowsByNode(items: JunkItem[]): Map<number, JunkItem> {
  return new Map(items.flatMap((item) => item.ids.map((id) => [id, item] as const)))
}

/** Dry run: what a delete of these items would do. Removes nothing. */
export function previewDelete(ids: number[]): Promise<DeletePreview> {
  return invoke<DeletePreview>('cleanup_delete_preview', { ids })
}

/**
 * Deletes the items one by one, then refreshes the totals and the shown folder. Resolves with
 * a result for each item that went ahead.
 */
export async function removeMany(
  ids: number[],
  permanent: boolean,
  confirmToken?: string,
): Promise<ItemResult[]> {
  const done = await trackJob(() =>
    invoke<DeleteManyResult>('cleanup_delete_many', { ids, permanent, confirmToken }),
  )
  cleanupJob.overview = done.overview
  const notices = [done.logError, done.overview.warning].filter(Boolean)
  if (notices.length > 0) {
    cleanupJob.notice = notices.join(' ')
  }
  cleanupJob.trail = [done.overview.root, ...cleanupJob.trail.slice(1)]
  const shown = cleanupJob.trail.at(-1)
  if (shown) {
    const seq = ++navSeq
    const rows = await invoke<CleanupRow[]>('cleanup_children', { id: shown.id })
    if (seq === navSeq) {
      cleanupJob.rows = rows
    }
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

export type RestoreOutcome = 'restored' | 'exists' | 'notInBin' | 'permanent' | 'failed'

export interface RestoreResult {
  path: string
  size: number
  outcome: RestoreOutcome
  message: string | null
}

/** The last bulk delete in the deletion log. */
export interface LastRun {
  time: string
  items: number
  /** Of those, the ones that went to the Recycle Bin. */
  recycled: number
}

export function loadLastRun(): Promise<LastRun | null> {
  return invoke<LastRun | null>('cleanup_last_run')
}

/** Puts the last bulk delete back from the Recycle Bin; the scan is out of date afterwards. */
export function restoreLast(): Promise<RestoreResult[]> {
  return invoke<RestoreResult[]>('cleanup_restore_last')
}

/** One line for the result of a restore. */
export function restoreSentence(results: RestoreResult[]): string {
  const count = (outcome: RestoreOutcome): number =>
    results.filter((result) => result.outcome === outcome).length
  const restored = count('restored')
  const left = results.length - restored
  const base = `${String(restored)} of ${String(results.length)} restored`
  return left > 0 ? `${base}; ${String(left)} could not be put back.` : `${base}.`
}
