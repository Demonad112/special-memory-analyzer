import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }))
const dialog = vi.hoisted(() => ({ save: vi.fn(), open: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => core)
vi.mock('@tauri-apps/plugin-dialog', () => dialog)

import { brand, csvJobDetails } from './job'
import {
  clearLogo,
  deleteReport,
  exportFileName,
  exportReport,
  loadLogo,
  openReport,
  pickLogo,
  pruneReports,
  retentionDays,
  type ReportEntry,
} from './reports'

const entry: ReportEntry = {
  id: 'compare-20261003-1',
  kind: 'compare',
  createdAtMs: new Date(2026, 9, 3, 14, 5).getTime(),
  title: 'Compare',
  subject: 'C:\\A → D:\\A',
  headline: 'Nothing missing',
  problem: false,
  job: { client: 'Acme: West', ticket: '', technician: '' },
}

beforeEach(() => {
  vi.clearAllMocks()
  brand.value = 'Acme IT'
  csvJobDetails.value = true
})

describe('reports', () => {
  it('names exports by client, kind and time, without characters Windows rejects', () => {
    expect(exportFileName(entry, 'html', entry.job.client)).toBe(
      'Acme- West Compare 2026-10-03 1405.html',
    )
    expect(exportFileName(entry, 'csv')).toBe('Compare 2026-10-03 1405.csv')
  })

  it('exports to the chosen file with the company name', async () => {
    dialog.save.mockResolvedValueOnce('C:\\out\\r.html')
    await expect(exportReport(entry, 'html')).resolves.toBe('C:\\out\\r.html')
    expect(core.invoke).toHaveBeenCalledWith('report_export', {
      kind: 'compare',
      id: entry.id,
      format: 'html',
      path: 'C:\\out\\r.html',
      brand: 'Acme IT',
      jobDetails: true,
    })
  })

  it('sends the CSV job details setting', async () => {
    csvJobDetails.value = false
    dialog.save.mockResolvedValueOnce('C:\\out\\r.csv')
    await exportReport(entry, 'csv')
    expect(core.invoke).toHaveBeenCalledWith(
      'report_export',
      expect.objectContaining({ format: 'csv', jobDetails: false }),
    )
  })

  it('hands the picked logo path to Rust and returns its preview', async () => {
    dialog.open.mockResolvedValueOnce('C:\\Pictures\\acme.png')
    core.invoke.mockResolvedValueOnce('data:image/png;base64,iVBO')
    await expect(pickLogo()).resolves.toBe('data:image/png;base64,iVBO')
    expect(dialog.open).toHaveBeenCalledWith(
      expect.objectContaining({
        multiple: false,
        directory: false,
        filters: [{ name: 'PNG or JPEG image', extensions: ['png', 'jpg', 'jpeg'] }],
      }),
    )
    expect(core.invoke).toHaveBeenCalledWith('branding_set_logo', {
      path: 'C:\\Pictures\\acme.png',
    })

    dialog.open.mockResolvedValueOnce(null)
    await expect(pickLogo()).resolves.toBeNull()
    expect(core.invoke).toHaveBeenCalledTimes(1)

    core.invoke.mockResolvedValueOnce(null)
    await expect(loadLogo()).resolves.toBeNull()
    expect(core.invoke).toHaveBeenLastCalledWith('branding_logo')
    await clearLogo()
    expect(core.invoke).toHaveBeenLastCalledWith('branding_clear_logo')
  })

  it('does nothing when the save dialog is cancelled', async () => {
    dialog.save.mockResolvedValueOnce(null)
    await expect(exportReport(entry, 'csv')).resolves.toBeNull()
    expect(core.invoke).not.toHaveBeenCalled()
  })

  it('opens and deletes by kind and id', async () => {
    await openReport(entry)
    expect(core.invoke).toHaveBeenCalledWith('report_open', {
      kind: 'compare',
      id: entry.id,
      brand: 'Acme IT',
    })
    await deleteReport({ kind: 'record', id: 'r1' })
    expect(core.invoke).toHaveBeenLastCalledWith('report_delete', { kind: 'record', id: 'r1' })
  })

  it('prunes only when a retention is chosen, and never throws', async () => {
    retentionDays.value = 0
    expect(await pruneReports()).toBe(0)
    expect(core.invoke).not.toHaveBeenCalled()

    retentionDays.value = 90
    core.invoke.mockResolvedValueOnce(4)
    expect(await pruneReports()).toBe(4)
    expect(core.invoke).toHaveBeenLastCalledWith('reports_prune', { days: 90 })

    core.invoke.mockRejectedValueOnce(new Error('locked'))
    expect(await pruneReports()).toBe(0)
    retentionDays.value = 0
  })
})
