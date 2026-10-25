import { Channel, invoke } from '@tauri-apps/api/core'
import { shallowReactive } from 'vue'
import { trackJob } from './busy'
import type { EntryKind, ScanProgress } from './compare'
import { errorMessage } from './errors'
import { currentJob } from './job'

export type TargetProfile = 'fileServer' | 'sharePoint'
export type ProblemKind = 'name' | 'longPath' | 'inUse' | 'unreadable' | 'link' | 'other'

export const targetLabels: Record<TargetProfile, string> = {
  fileServer: 'File server (NTFS)',
  sharePoint: 'OneDrive / SharePoint',
}

/** The groups in the order the screen and the report show them. */
export const kindHeadings: Record<ProblemKind, string> = {
  name: 'Names',
  longPath: 'Long paths',
  inUse: 'In use',
  unreadable: 'Could not be read',
  link: 'Links (not followed)',
  other: 'Other',
}

export interface RowProblem {
  kind: ProblemKind
  rule: string
  label: string
  hint: string
  /** The item cannot be created at the target; the rest are warnings. */
  blocker: boolean
  /** In use: the programs Restart Manager named. */
  holders: { app: string; pid: number }[]
}

/** A file or folder with problems at or below it. */
export interface ReadinessRow {
  id: number
  name: string
  kind: EntryKind
  problems: number
  own: RowProblem[]
  hasChildren: boolean
}

export interface RuleCount {
  kind: ProblemKind
  rule: string
  count: number
  blocker: boolean
}

export interface ReadinessReport {
  source: string
  destinationRoot: string
  target: TargetProfile
  files: number
  dirs: number
  size: number
  cloudFiles: number
  links: number
  unreadable: number
  nameProblems: number
  longPaths: number
  blockers: number
  problemsTotal: number
  elapsedMs: number
  counts: RuleCount[]
  inUse: InUseSummary
  /** Owners and access lists. The findings stay in Rust: see `loadFindings`. */
  permissions: PermissionsSummary
  permissionIssues: number
}

export type FindingKind =
  'brokenInheritance' | 'unknownAccount' | 'wideWrite' | 'explicitFile' | 'unreadable'

/** The findings in the order the panel shows them. */
export const findingHeadings: Record<FindingKind, string> = {
  brokenInheritance: 'Inheritance broken',
  unknownAccount: 'Unknown accounts',
  wideWrite: 'Wide write access',
  explicitFile: 'Files with their own entries',
  unreadable: 'Permissions not readable',
}

export const findingHints: Record<FindingKind, string> = {
  brokenInheritance:
    'These folders do not take permissions from their parent. Copy them with their permissions and check the new parent grants what is expected.',
  unknownAccount:
    'The account was deleted, or lives in a domain this PC cannot see. The entry does nothing and will not map to anyone at the target.',
  wideWrite:
    'Anyone in Everyone, Authenticated Users, Users, Domain Users or Anonymous can change files here.',
  explicitFile: 'Permissions set on single files are easy to lose or forget in a migration.',
  unreadable: 'Run as administrator to read more, or fix the folder permissions first.',
}

/** Findings that call for action; the other two describe how the share is built. */
export const issueKinds: FindingKind[] = ['unknownAccount', 'wideWrite', 'unreadable']

export interface FindingCount {
  kind: FindingKind
  count: number
}

export interface PermissionsSummary {
  status: 'off' | 'checked' | 'notSupported' | 'other'
  everyFile: boolean
  elevated: boolean
  /** The server account names were asked of; empty for this PC. */
  server: string
  /** Said when a mapped drive's server could not be found, so names were asked of this PC. */
  serverNote?: string
  foldersRead: number
  filesRead: number
  skipped: number
  issues: number
  counts: FindingCount[]
  itemsTruncated: boolean
}

export interface Finding {
  /** Relative to the source; empty for the source folder. */
  path: string
  dir: boolean
  kind: FindingKind
  detail: string
  sid: string
}

export interface FindingsPage {
  total: number
  findings: Finding[]
}

export interface InUseSummary {
  status: 'off' | 'checked' | 'notSupported' | 'other'
  checked: number
  inUse: number
  unreadable: number
  /** Links and online-only files: never opened. */
  skipped: number
  holdersAsked: number
}

export interface ReadinessProgress {
  phase: 'scan' | 'names' | 'inUse' | 'permissions'
  scan: ScanProgress
  /** In-use pass: files to open, opened so far, found in use. */
  toCheck: number
  checked: number
  inUse: number
  /** Permissions pass: items to read and read so far. */
  permToCheck: number
  permChecked: number
}

export interface ReadinessResult {
  report: ReadinessReport
  /** The source folder itself, with its own problems. */
  root: ReadinessRow
  rows: ReadinessRow[]
  reportId: string | null
}

type ReadinessEvent = { kind: 'progress'; progress: ReadinessProgress }

/** The error text the backend returns for a cancelled check. */
export const CANCELLED = 'cancelled'

export function startReadiness(
  form: ReadinessForm,
  onProgress: (progress: ReadinessProgress) => void,
): Promise<ReadinessResult> {
  const onEvent = new Channel<ReadinessEvent>()
  onEvent.onmessage = (event) => {
    onProgress(event.progress)
  }
  return trackJob(() =>
    invoke<ReadinessResult>('readiness_start', {
      path: form.path,
      destination: form.destination,
      target: form.target,
      checkInUse: form.checkInUse,
      checkPermissions: form.checkPermissions,
      everyFile: form.everyFile,
      job: currentJob(),
      onEvent,
    }),
  )
}

export function cancelReadiness(): Promise<void> {
  return invoke('readiness_cancel')
}

export function loadChildren(id: number, kind: ProblemKind | null): Promise<ReadinessRow[]> {
  return invoke<ReadinessRow[]>('readiness_children', { id, kind })
}

export function loadRoot(kind: ProblemKind | null): Promise<ReadinessRow> {
  return invoke<ReadinessRow>('readiness_root', { kind })
}

/** One page of the permission findings of one type. */
export function loadFindings(
  kind: FindingKind,
  offset: number,
  limit: number,
): Promise<FindingsPage> {
  return invoke<FindingsPage>('readiness_permissions', { kind, offset, limit })
}

export function revealItem(id: number): Promise<void> {
  return invoke('readiness_reveal', { id })
}

export interface Group {
  kind: ProblemKind
  heading: string
  count: number
  blockers: number
}

/** Problems per group, in screen order, leaving out empty groups. */
export function groupCounts(report: Pick<ReadinessReport, 'counts'>): Group[] {
  const order = Object.keys(kindHeadings) as ProblemKind[]
  return order
    .map((kind) => {
      const counts = report.counts.filter((count) => count.kind === kind)
      return {
        kind,
        heading: kindHeadings[kind],
        count: counts.reduce((sum, count) => sum + count.count, 0),
        blockers: counts
          .filter((count) => count.blocker)
          .reduce((sum, count) => sum + count.count, 0),
      }
    })
    .filter((group) => group.count > 0)
}

/** A name with a space at either end is quoted, or the space would not show. */
export function displayName(name: string): string {
  return /^\s|\s$/.test(name) ? `"${name}"` : name
}

export interface VisibleRow {
  row: ReadinessRow
  depth: number
}

/** The rows on screen: the top level, then each open folder's loaded children below it. */
export function visibleRows(
  top: ReadinessRow[],
  children: Map<number, ReadinessRow[]>,
  expanded: Set<number>,
): VisibleRow[] {
  const out: VisibleRow[] = []
  const stack: VisibleRow[] = top.map((row) => ({ row, depth: 0 })).reverse()
  while (stack.length > 0) {
    const next = stack.pop()
    if (!next) {
      break
    }
    out.push(next)
    if (expanded.has(next.row.id)) {
      const below = children.get(next.row.id) ?? []
      for (const row of [...below].reverse()) {
        stack.push({ row, depth: next.depth + 1 })
      }
    }
  }
  return out
}

export interface ReadinessForm {
  path: string
  destination: string
  target: TargetProfile
  /** Open every file to find the ones in use. On unless turned off. */
  checkInUse: boolean
  /** Read the owner and access list of every folder. On unless turned off. */
  checkPermissions: boolean
  /** Read every file's permissions too, not just folders. Off unless turned on. */
  everyFile: boolean
}

const formKey = 'analyzer-readiness'

export function loadForm(): ReadinessForm {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(formKey) ?? 'null')
    if (saved && typeof saved === 'object') {
      const { path, destination, target, checkInUse, checkPermissions, everyFile } =
        saved as Record<string, unknown>
      return {
        path: typeof path === 'string' ? path : '',
        destination: typeof destination === 'string' ? destination : '',
        target: target === 'sharePoint' ? 'sharePoint' : 'fileServer',
        checkInUse: checkInUse !== false,
        checkPermissions: checkPermissions !== false,
        everyFile: everyFile === true,
      }
    }
  } catch {
    // Unreadable storage: start empty.
  }
  return {
    path: '',
    destination: '',
    target: 'fileServer',
    checkInUse: true,
    checkPermissions: true,
    everyFile: false,
  }
}

export function saveForm(form: ReadinessForm): void {
  try {
    localStorage.setItem(formKey, JSON.stringify(form))
  } catch {
    // Not remembered.
  }
}

/**
 * The check in progress or last finished. It lives here, not in the page, so leaving the page
 * mid-check neither loses the result nor allows a second check while the backend is busy.
 * `view` is bumped whenever the rows on screen are replaced (new result, new filter); a load
 * that started under an older view is dropped.
 */
export const readinessJob = shallowReactive({
  running: false,
  progress: null as ReadinessProgress | null,
  result: null as ReadinessResult | null,
  filter: null as ProblemKind | null,
  root: null as ReadinessRow | null,
  rows: [] as ReadinessRow[],
  children: new Map<number, ReadinessRow[]>(),
  expanded: new Set<number>(),
  loading: new Set<number>(),
  view: 0,
  error: '',
  notice: '',
  startedAt: 0,
  finishedAt: null as Date | null,
})

/** Runs a check into `readinessJob`. Does nothing while one is already running. */
export async function runReadinessJob(form: ReadinessForm): Promise<void> {
  if (readinessJob.running) {
    return
  }
  Object.assign(readinessJob, {
    running: true,
    error: '',
    notice: '',
    progress: null,
    result: null,
    filter: null,
    root: null,
    rows: [],
    children: new Map(),
    expanded: new Set(),
    loading: new Set(),
    view: readinessJob.view + 1,
    startedAt: Date.now(),
  })
  try {
    const result = await startReadiness(form, (update) => {
      readinessJob.progress = update
    })
    Object.assign(readinessJob, {
      result,
      root: result.root,
      rows: result.rows,
      finishedAt: new Date(),
      view: readinessJob.view + 1,
    })
  } catch (err) {
    if (err === CANCELLED) {
      readinessJob.notice = 'Check cancelled. Nothing was changed.'
    } else {
      readinessJob.error = errorMessage(err)
    }
  } finally {
    readinessJob.running = false
  }
}

/** Shows only one group (or all with `null`): reloads the top rows and closes every folder. */
export async function setFilter(kind: ProblemKind | null): Promise<void> {
  if (!readinessJob.result || readinessJob.filter === kind) {
    return
  }
  const view = readinessJob.view + 1
  Object.assign(readinessJob, {
    filter: kind,
    view,
    children: new Map(),
    expanded: new Set(),
    loading: new Set(),
  })
  const [root, rows] = await Promise.all([loadRoot(kind), loadChildren(0, kind)])
  if (readinessJob.view === view) {
    Object.assign(readinessJob, { root, rows })
  }
}

/**
 * Opens or closes a folder row, loading its children on first open. Rows that arrive after the
 * view changed (a new check or filter) are dropped. Rejects if the children cannot be loaded.
 */
export async function toggleRow(id: number): Promise<void> {
  const next = new Set(readinessJob.expanded)
  if (next.has(id)) {
    next.delete(id)
    readinessJob.expanded = next
    return
  }
  next.add(id)
  readinessJob.expanded = next
  if (readinessJob.children.has(id) || readinessJob.loading.has(id)) {
    return
  }
  const view = readinessJob.view
  readinessJob.loading = new Set(readinessJob.loading).add(id)
  try {
    const loaded = await loadChildren(id, readinessJob.filter)
    if (readinessJob.view === view) {
      readinessJob.children = new Map(readinessJob.children).set(id, loaded)
    }
  } catch (err) {
    if (readinessJob.view === view) {
      const collapsed = new Set(readinessJob.expanded)
      collapsed.delete(id)
      readinessJob.expanded = collapsed
    }
    throw err
  } finally {
    if (readinessJob.view === view) {
      const done = new Set(readinessJob.loading)
      done.delete(id)
      readinessJob.loading = done
    }
  }
}
