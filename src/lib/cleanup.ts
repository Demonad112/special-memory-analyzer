import { Channel, invoke, isTauri } from '@tauri-apps/api/core'
import { reactive, ref, watch } from 'vue'
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

/** The item the actions apply to, from the list, the treemap or "Largest files". */
export interface PickedItem {
  id: number
  name: string
  size: number
  isFolder: boolean
  files: number
}

export const HARD_LINK_TITLE =
  'Another name for this file is in the scan. Only one name carries the size on disk.'

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
  /** When set, folders list only the files this owner owns (see `OwnersPanel`). */
  ownerFilter: null as OwnerFilter | null,
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

const HISTORY_KEY = 'analyzer-keep-scan-history'

function readKeepHistory(): boolean {
  try {
    return localStorage.getItem(HISTORY_KEY) !== 'off'
  } catch {
    return true
  }
}

/** Settings > Keep scan history: every completed scan saves a snapshot. On by default. */
export const keepHistory = ref(readKeepHistory())

watch(keepHistory, (on) => {
  try {
    localStorage.setItem(HISTORY_KEY, on ? 'on' : 'off')
  } catch {
    // Not remembered.
  }
})

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
    ownerFilter: null,
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
      history: keepHistory.value,
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

/** Which owner the folder view is limited to: an index of the owner table. */
export interface OwnerFilter {
  index: number
  name: string
}

/** The command and arguments that list a folder, limited to one owner when a filter is set. */
export function childrenCall(
  id: number,
  filter: OwnerFilter | null,
): { command: string; args: Record<string, number> } {
  return filter
    ? { command: 'cleanup_owner_children', args: { id, owner: filter.index } }
    : { command: 'cleanup_children', args: { id } }
}

function childrenOf(id: number): Promise<CleanupRow[]> {
  const { command, args } = childrenCall(id, cleanupJob.ownerFilter)
  return invoke<CleanupRow[]>(command, args)
}

/** Limits the folder view to one owner's files, or lifts the limit; shows the same folder again. */
export async function setOwnerFilter(filter: OwnerFilter | null): Promise<void> {
  cleanupJob.ownerFilter = filter
  const shown = cleanupJob.trail.at(-1)
  if (!shown) {
    return
  }
  const seq = ++navSeq
  const rows = await childrenOf(shown.id)
  if (seq === navSeq) {
    cleanupJob.rows = rows
  }
}

/** Shows what is inside `row`, a folder in the shown one. */
export async function openFolder(row: CleanupRow): Promise<void> {
  const seq = ++navSeq
  const rows = await childrenOf(row.id)
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
  const rows = await childrenOf(folder.id)
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
    const rows = await childrenOf(shown.id)
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

export type PreviewStatus = 'go' | 'protected' | 'reparse' | 'badName' | 'gone' | 'inside'

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

/**
 * The command behind a bulk delete. Duplicates have their own, which refuses to delete every copy
 * of a group and re-checks the files first.
 */
export type DeleteCommand = 'cleanup_delete_many' | 'cleanup_delete_duplicates'

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
  command: DeleteCommand = 'cleanup_delete_many',
): Promise<ItemResult[]> {
  const done = await trackJob(() =>
    invoke<DeleteManyResult>(command, { ids, permanent, confirmToken }),
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
    const rows = await childrenOf(shown.id)
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

const KIB = 1024
const MIB = 1024 * KIB

/** The minimum sizes the duplicate finder offers; 1 MiB is the default. */
export const DUPE_MIN_SIZES = [64 * KIB, MIB, 10 * MIB, 100 * MIB]
export const DEFAULT_DUPE_MIN = MIB

export type DupeStage = 'sizes' | 'ends' | 'full' | 'done'

export interface DupeProgress {
  stage: DupeStage
  /** Files finished in this stage, of `total`. */
  done: number
  total: number
  /** Bytes read so far. */
  bytes: number
}

export interface DupeMember {
  id: number
  name: string
  folder: string
  modifiedMs: number | null
  /** Space on disk for this copy. */
  disk: number
}

export interface DupeGroup {
  id: number
  /** Length of each copy. */
  size: number
  /** Space freed by keeping one copy. */
  reclaimable: number
  /** Oldest first. */
  members: DupeMember[]
}

export interface DupesSummary {
  minSize: number
  groups: number
  files: number
  reclaimable: number
  skipped: { cloud: number; hardLinked: number; unreadable: number }
}

/** Groups loaded per page. */
export const DUPE_PAGE = 100

/** Searches the current scan for files with identical content; progress about every 100 ms. */
export function findDuplicates(
  minSize: number,
  progress: (progress: DupeProgress) => void,
): Promise<DupesSummary> {
  const onProgress = new Channel<DupeProgress>()
  onProgress.onmessage = progress
  return trackJob(() => invoke<DupesSummary>('cleanup_dupes_find', { minSize, onProgress }))
}

export function cancelDuplicates(): Promise<void> {
  return invoke('cleanup_dupes_cancel')
}

export function loadDuplicateGroups(offset: number, limit = DUPE_PAGE): Promise<DupeGroup[]> {
  return invoke<DupeGroup[]>('cleanup_dupes_groups', { offset, limit })
}

/** The totals of the last search, after deletes; null when there is none. */
export function loadDuplicateSummary(): Promise<DupesSummary | null> {
  return invoke<DupesSummary | null>('cleanup_dupes_summary')
}

/** Every copy but the oldest in each group, for the button that ticks them. */
export function allButOldest(): Promise<number[]> {
  return invoke<number[]>('cleanup_dupes_all_but_oldest')
}

const DUPE_STAGES: Record<DupeStage, string> = {
  sizes: 'Listing files of the same size',
  ends: 'Comparing the start and end of each file',
  full: 'Comparing whole files',
  done: 'Finishing',
}

/** One line for the progress of a duplicate search. */
export function dupeProgressLine(progress: DupeProgress | null): string {
  if (!progress) {
    return 'Starting…'
  }
  const label = DUPE_STAGES[progress.stage]
  return progress.total > 0
    ? `${label}: ${String(progress.done)} of ${String(progress.total)}…`
    : `${label}…`
}

/** Groups (of those loaded) in which every copy is ticked. Rust refuses those anyway. */
export function groupsWithoutCopy(groups: DupeGroup[], ticked: ReadonlySet<number>): DupeGroup[] {
  return groups.filter((group) => group.members.every((member) => ticked.has(member.id)))
}

/** Space the ticked copies take on disk, over the loaded groups. */
export function tickedDisk(groups: DupeGroup[], ticked: ReadonlySet<number>): number {
  return groups
    .flatMap((group) => group.members)
    .filter((member) => ticked.has(member.id))
    .reduce((sum, member) => sum + member.disk, 0)
}

export type OwnerKind = 'known' | 'notFound' | 'unreadable'

export interface OwnerFolder {
  id: number
  name: string
  disk: number
}

export interface OwnerRow {
  /** What `OwnerFilter.index` takes. */
  index: number
  sid: string | null
  name: string
  kind: OwnerKind
  files: number
  /** Space on disk. */
  disk: number
  topFolders: OwnerFolder[]
}

export interface OwnerTable {
  totalDisk: number
  totalFiles: number
  rows: OwnerRow[]
}

export interface OwnerProgress {
  done: number
  total: number
}

/** Reads the owner of every file in the scan (on demand); progress about every 100 ms. */
export function readOwners(progress: (progress: OwnerProgress) => void): Promise<OwnerTable> {
  const onProgress = new Channel<OwnerProgress>()
  onProgress.onmessage = progress
  return trackJob(() => invoke<OwnerTable>('cleanup_owners_read', { onProgress }))
}

export function cancelOwners(): Promise<void> {
  return invoke('cleanup_owners_cancel')
}

/** The table as it is after deletes; null when the pass has not run on this scan. */
export function loadOwnerTable(): Promise<OwnerTable | null> {
  return invoke<OwnerTable | null>('cleanup_owners_table')
}

/** One line for the progress of the owner pass. */
export function ownerProgressLine(progress: OwnerProgress | null): string {
  if (!progress || progress.total === 0) {
    return 'Reading owners…'
  }
  return `Reading owners: ${String(progress.done)} of ${String(progress.total)} files…`
}

export interface SnapshotInfo {
  takenMs: number
  size: number
  driveUsed: number | null
  driveTotal: number | null
  /** Saved by the scan on screen. */
  current: boolean
}

export type Forecast =
  | { kind: 'notEnoughHistory' }
  | { kind: 'notGrowing' }
  | { kind: 'hidden' }
  | { kind: 'full'; days: number; perDay: number; windowDays: number; points: number }

export interface History {
  root: string
  /** Newest first. */
  snapshots: SnapshotInfo[]
  forecast: Forecast
  nowMs: number
}

export type GrowthChange = 'grown' | 'shrunk' | 'new' | 'removed'

export interface GrowthRow {
  path: string
  change: GrowthChange
  before: number
  after: number
  delta: number
  /** Of the earlier size; null for a new folder. */
  percent: number | null
}

export interface Growth {
  fromMs: number
  toMs: number
  before: number
  after: number
  delta: number
  rows: GrowthRow[]
  /** Changed folders beyond the rows listed. */
  hidden: number
}

export interface RootHistory {
  key: string
  root: string
  count: number
  firstMs: number
  lastMs: number
}

export function loadHistory(): Promise<History> {
  return invoke<History>('cleanup_history')
}

/** What grew since the snapshot taken at `fromMs` (the previous scan when omitted). */
export function loadGrowth(fromMs?: number): Promise<Growth> {
  return invoke<Growth>('cleanup_growth', { fromMs: fromMs ?? null })
}

export function loadHistoryRoots(): Promise<RootHistory[]> {
  return invoke<RootHistory[]>('history_roots')
}

/** Deletes the history of one folder, or of every folder without a key. */
export function deleteHistory(key?: string): Promise<number> {
  return invoke<number>('history_delete', { key: key ?? null })
}

/** The forecast line, or null when nothing should be said (more than 5 years away). */
export function forecastLine(forecast: Forecast): string | null {
  switch (forecast.kind) {
    case 'notEnoughHistory':
      return 'Not enough history yet: a forecast needs 3 scans over at least 2 weeks.'
    case 'notGrowing':
      return 'Not growing: this drive has not been filling up over the last 90 days.'
    case 'hidden':
      return null
    case 'full':
      return forecast.days === 0
        ? `At the last ${String(forecast.windowDays)} days' rate, this drive is full about now.`
        : `At the last ${String(forecast.windowDays)} days' rate, this drive is full in about ${String(forecast.days)} ${forecast.days === 1 ? 'day' : 'days'}.`
  }
}

const GROWTH_LABELS: Record<GrowthChange, string> = {
  grown: 'Grew',
  shrunk: 'Shrank',
  new: 'New',
  removed: 'Gone',
}

export function growthLabel(change: GrowthChange): string {
  return GROWTH_LABELS[change]
}

/** The ages the stale report offers, in years; 3 is the default. */
export const STALE_YEARS = [1, 2, 3, 5, 7, 10]
export const DEFAULT_STALE_YEARS = 3

/** Whether Windows keeps last-access times on NTFS on this PC. */
export type LastAccess = 'kept' | 'off' | 'unknown'

export interface StaleGroup {
  /** The first-level folder's id; 0 for the files directly in the scanned folder. */
  id: number
  name: string
  isFolder: boolean
  staleFiles: number
  staleBytes: number
  folderBytes: number
  newestMs: number | null
}

export interface StaleReport {
  years: number
  notOpened: boolean
  nowMs: number
  groups: StaleGroup[]
  staleFiles: number
  staleBytes: number
  unknownFiles: number
  unknownBytes: number
  cloudFiles: number
  totalBytes: number
  lastAccess: LastAccess
}

export interface StaleFile {
  id: number
  /** Below the scanned folder. */
  path: string
  size: number
  modifiedMs: number | null
  accessedMs: number | null
}

export function loadStale(years: number, notOpened: boolean): Promise<StaleReport> {
  return invoke<StaleReport>('cleanup_stale', { years, notOpened })
}

export function loadStaleFiles(
  group: number,
  years: number,
  notOpened: boolean,
): Promise<StaleFile[]> {
  return invoke<StaleFile[]>('cleanup_stale_files', { group, years, notOpened })
}

/** The note under "Also not opened": access times are only as good as Windows keeps them. */
export function lastAccessNote(lastAccess: LastAccess): string {
  switch (lastAccess) {
    case 'off':
      return 'Windows is not keeping last-access times on this PC (NtfsDisableLastAccessUpdate), so "not opened" is unreliable here.'
    case 'unknown':
      return 'Could not check whether Windows keeps last-access times; treat "not opened" as a hint.'
    case 'kept':
      return 'Windows keeps last-access times on this PC, but programs such as backup or antivirus can update them.'
  }
}
