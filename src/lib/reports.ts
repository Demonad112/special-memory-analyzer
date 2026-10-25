import { invoke, isTauri } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { ref, watch } from 'vue'
import type { JobInfo } from './job'
import { brand, csvJobDetails } from './job'

export type ReportKind = 'compare' | 'record' | 'diskUsage' | 'cleanup' | 'readiness' | 'runbook'
export type ExportFormat = 'html' | 'csv'

/** One saved report or record, as listed on the Reports page. */
export interface ReportEntry {
  id: string
  kind: ReportKind
  createdAtMs: number
  title: string
  subject: string
  headline: string
  problem: boolean
  job: JobInfo
  /** Other CSV layouts the report can export, by name (`permissions` for an audited readiness report). */
  csvLayouts: string[]
}

export const kindLabels: Record<ReportKind, string> = {
  compare: 'Compare',
  record: 'Record',
  diskUsage: 'Disk usage',
  cleanup: 'Cleanup',
  readiness: 'Readiness',
  runbook: 'Runbook',
}

export function listReports(): Promise<ReportEntry[]> {
  return invoke<ReportEntry[]>('reports_list')
}

export function deleteReport(entry: Pick<ReportEntry, 'kind' | 'id'>): Promise<void> {
  return invoke('report_delete', { kind: entry.kind, id: entry.id })
}

const retentionKey = 'analyzer-report-retention-days'

/** How long Compare, Disk usage and Cleanup reports are kept; 0 keeps them for ever. */
export const retentionChoices = [
  { days: 0, label: 'Keep for ever' },
  { days: 30, label: '30 days' },
  { days: 90, label: '90 days' },
  { days: 365, label: '1 year' },
] as const

function readRetention(): number {
  try {
    const days = Number(localStorage.getItem(retentionKey))
    return retentionChoices.some((choice) => choice.days === days) ? days : 0
  } catch {
    return 0
  }
}

export const retentionDays = ref(readRetention())

watch(retentionDays, (days) => {
  try {
    localStorage.setItem(retentionKey, String(days))
  } catch {
    // Not remembered.
  }
})

/** Deletes reports older than the chosen retention; record runs are never pruned. */
export async function pruneReports(): Promise<number> {
  if (retentionDays.value === 0 || !isTauri()) {
    return 0
  }
  try {
    return await invoke<number>('reports_prune', { days: retentionDays.value })
  } catch {
    return 0
  }
}

/** Opens the report as a web page in the default browser, headed with the company name. */
export function openReport(entry: Pick<ReportEntry, 'kind' | 'id'>): Promise<void> {
  return invoke('report_open', { kind: entry.kind, id: entry.id, brand: brand.value })
}

/** "Compare 2026-10-03 1432.html": a file name Windows accepts. */
export function exportFileName(
  entry: Pick<ReportEntry, 'title' | 'createdAtMs'>,
  format: ExportFormat,
  client = '',
  layout = '',
): string {
  const date = new Date(entry.createdAtMs)
  const pad = (value: number): string => String(value).padStart(2, '0')
  const stamp = `${String(date.getFullYear())}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}${pad(date.getMinutes())}`
  const name = [client.trim(), entry.title, layout, stamp].filter(Boolean).join(' ')
  return `${name.replace(/[<>:"/\\|?*]+/g, '-')}.${format}`
}

/**
 * Asks where to save, then writes the report there. Resolves to the saved path, or `null`
 * when the dialog was cancelled.
 */
export async function exportReport(
  entry: Pick<ReportEntry, 'kind' | 'id' | 'title' | 'createdAtMs' | 'job'>,
  format: ExportFormat,
  layout?: string,
): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  const path = await save({
    title: format === 'html' ? 'Save the report' : 'Save the list for Excel',
    defaultPath: exportFileName(entry, format, entry.job.client, layout),
    filters: [
      format === 'html'
        ? { name: 'Web page', extensions: ['html'] }
        : { name: 'CSV (Excel)', extensions: ['csv'] },
    ],
  })
  if (!path) {
    return null
  }
  await invoke('report_export', {
    kind: entry.kind,
    id: entry.id,
    format,
    path,
    brand: brand.value,
    jobDetails: csvJobDetails.value,
    ...(layout ? { layout } : {}),
  })
  return path
}

/** The saved company logo as a `data:` URI, or `null` when there is none. */
export async function loadLogo(): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  return invoke<string | null>('branding_logo')
}

/**
 * Asks for a PNG or JPEG; Rust checks it (at most 256 KB, no SVG) and keeps its own copy.
 * Resolves to the new logo as a `data:` URI, or `null` when the dialog was cancelled.
 */
export async function pickLogo(): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  const path = await open({
    title: 'Choose the company logo',
    multiple: false,
    directory: false,
    filters: [{ name: 'PNG or JPEG image', extensions: ['png', 'jpg', 'jpeg'] }],
  })
  if (typeof path !== 'string') {
    return null
  }
  return invoke<string>('branding_set_logo', { path })
}

export function clearLogo(): Promise<void> {
  return invoke('branding_clear_logo')
}

/** One way to export a report as CSV: the default list, or another layout of the same report. */
export interface CsvChoice {
  /** Passed to `report_export`; `undefined` is the default layout. */
  layout: string | undefined
  label: string
  title: string
}

const layoutChoices: Record<string, Omit<CsvChoice, 'layout'>> = {
  permissions: {
    label: 'Export the permissions as CSV',
    title: 'Save every folder and its access entries as CSV for Excel',
  },
}

/**
 * The CSV exports to offer for a report: always the default list, plus each other layout the
 * report was saved with. A readiness report saved without the permissions audit has none, so it
 * gets no permissions CSV.
 */
export function csvChoices(entry: Pick<ReportEntry, 'csvLayouts'>): CsvChoice[] {
  const extra = (entry.csvLayouts as string[] | undefined) ?? []
  return [
    {
      layout: undefined,
      label: 'Export as CSV',
      title: 'Save the list as CSV for Excel',
    },
    ...extra.flatMap((layout) => {
      const choice = layoutChoices[layout]
      return choice ? [{ layout, ...choice }] : []
    }),
  ]
}

/** The kinds a runbook can hold, in the order it reads: before the copy, the copy, after it. */
export const runbookKinds = ['readiness', 'record', 'compare'] as const
export type RunbookKind = (typeof runbookKinds)[number]

export const MAX_RUNBOOK_PARTS = 24

export function isRunbookKind(kind: ReportKind): kind is RunbookKind {
  return (runbookKinds as readonly string[]).includes(kind)
}

function sameTicket(a: JobInfo, b: JobInfo): boolean {
  const ticket = a.ticket.trim().toLowerCase()
  return ticket !== '' && ticket === b.ticket.trim().toLowerCase()
}

/**
 * The reports a runbook can be made from, with those that have the same ticket number as the
 * first one picked ahead of the rest (each group keeps its order, newest first). Before anything
 * is picked the list is just the eligible reports.
 */
export function runbookCandidates(
  entries: ReportEntry[],
  first: Pick<ReportEntry, 'job'> | null,
): { entry: ReportEntry; suggested: boolean }[] {
  const eligible = entries
    .filter((entry) => isRunbookKind(entry.kind))
    .map((entry) => ({ entry, suggested: first !== null && sameTicket(entry.job, first.job) }))
  return [...eligible.filter((row) => row.suggested), ...eligible.filter((row) => !row.suggested)]
}

/** Why the picked reports cannot be a runbook yet, or '' when they can. */
export function runbookProblem(picked: Pick<ReportEntry, 'kind'>[]): string {
  if (picked.length === 0) {
    return 'Pick at least one report.'
  }
  if (picked.filter((entry) => entry.kind === 'readiness').length > 1) {
    return 'A runbook has one readiness report.'
  }
  if (picked.length > MAX_RUNBOOK_PARTS) {
    return `A runbook holds at most ${String(MAX_RUNBOOK_PARTS)} reports.`
  }
  return ''
}

/** Client, ticket and technician of the first report picked: what the runbook is for. */
export function runbookJob(first: Pick<ReportEntry, 'job'> | undefined): JobInfo {
  return { ...(first?.job ?? { client: '', ticket: '', technician: '' }) }
}

/** Makes a runbook from the picked reports. Only ids cross; Rust checks each against its folders. */
export function createRunbook(
  info: JobInfo,
  picked: Pick<ReportEntry, 'kind' | 'id'>[],
): Promise<ReportEntry> {
  return invoke<ReportEntry>('runbook_create', {
    job: info,
    parts: picked.map((entry) => ({ kind: entry.kind, id: entry.id })),
  })
}
