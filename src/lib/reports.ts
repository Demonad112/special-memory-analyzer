import { invoke, isTauri } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { ref, watch } from 'vue'
import type { JobInfo } from './job'
import { brand, csvJobDetails } from './job'

export type ReportKind = 'compare' | 'record' | 'diskUsage' | 'cleanup' | 'readiness'
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
}

export const kindLabels: Record<ReportKind, string> = {
  compare: 'Compare',
  record: 'Record',
  diskUsage: 'Disk usage',
  cleanup: 'Cleanup',
  readiness: 'Readiness',
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
): string {
  const date = new Date(entry.createdAtMs)
  const pad = (value: number): string => String(value).padStart(2, '0')
  const stamp = `${String(date.getFullYear())}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}${pad(date.getMinutes())}`
  const name = [client.trim(), entry.title, stamp].filter(Boolean).join(' ')
  return `${name.replace(/[<>:"/\\|?*]+/g, '-')}.${format}`
}

/**
 * Asks where to save, then writes the report there. Resolves to the saved path, or `null`
 * when the dialog was cancelled.
 */
export async function exportReport(
  entry: Pick<ReportEntry, 'kind' | 'id' | 'title' | 'createdAtMs' | 'job'>,
  format: ExportFormat,
): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  const path = await save({
    title: format === 'html' ? 'Save the report' : 'Save the list for Excel',
    defaultPath: exportFileName(entry, format, entry.job.client),
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
