import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }))
const dialog = vi.hoisted(() => ({ save: vi.fn(), open: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => core)
vi.mock('@tauri-apps/plugin-dialog', () => dialog)

import { brand, csvJobDetails } from './job'
import {
  clearLogo,
  createRunbook,
  csvChoices,
  deleteReport,
  exportFileName,
  exportReport,
  loadLogo,
  openReport,
  pickLogo,
  pruneReports,
  retentionDays,
  runbookCandidates,
  runbookJob,
  runbookProblem,
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
  csvLayouts: [],
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

  it('exports another CSV layout under its own name', async () => {
    expect(exportFileName(entry, 'csv', '', 'permissions')).toBe(
      'Compare permissions 2026-10-03 1405.csv',
    )
    dialog.save.mockResolvedValueOnce('C:\\out\\p.csv')
    await exportReport(entry, 'csv', 'permissions')
    expect(core.invoke).toHaveBeenCalledWith(
      'report_export',
      expect.objectContaining({ format: 'csv', layout: 'permissions' }),
    )
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

describe('csv choices', () => {
  it('offers only the default list for reports without other layouts', () => {
    expect(csvChoices({ csvLayouts: [] }).map((choice) => choice.layout)).toEqual([undefined])
  })

  it('adds the permissions CSV for a readiness report saved with the audit', () => {
    const choices = csvChoices({ csvLayouts: ['permissions'] })
    expect(choices.map((choice) => choice.layout)).toEqual([undefined, 'permissions'])
    expect(choices[1]?.label).toBe('Export the permissions as CSV')
  })

  it('ignores a layout this version does not know', () => {
    expect(csvChoices({ csvLayouts: ['somethingNewer'] })).toHaveLength(1)
  })

  it('adds the permissions CSV for a compare report saved with the permissions check', async () => {
    const compare = {
      ...entry,
      kind: 'compare' as const,
      title: 'Compare',
      csvLayouts: ['permissions'],
    }
    const choices = csvChoices(compare)
    expect(choices.map((choice) => choice.layout)).toEqual([undefined, 'permissions'])
    // A compare saved without the check offers only the default list.
    expect(csvChoices({ ...compare, csvLayouts: [] })).toHaveLength(1)

    dialog.save.mockResolvedValueOnce('C:\\out\\c.csv')
    await exportReport(compare, 'csv', choices[1]?.layout)
    expect(core.invoke).toHaveBeenLastCalledWith(
      'report_export',
      expect.objectContaining({ kind: 'compare', format: 'csv', layout: 'permissions' }),
    )
  })

  it('exports the permissions layout from the Reports page choice', async () => {
    dialog.save.mockResolvedValueOnce('C:\\out\\p.csv')
    const readiness = { ...entry, kind: 'readiness' as const, title: 'Migration readiness' }
    const choice = csvChoices({ csvLayouts: ['permissions'] })[1]
    await exportReport(readiness, 'csv', choice?.layout)
    expect(core.invoke).toHaveBeenLastCalledWith(
      'report_export',
      expect.objectContaining({ kind: 'readiness', format: 'csv', layout: 'permissions' }),
    )
  })
})

describe('runbooks', () => {
  const row = (id: string, kind: ReportEntry['kind'], ticket: string): ReportEntry => ({
    ...entry,
    id,
    kind,
    job: { client: 'Acme', ticket, technician: '' },
  })
  const entries = [
    row('c-new', 'compare', 'T-2'),
    row('u-1', 'diskUsage', 'T-1'),
    row('r-new', 'record', 't-1 '),
    row('rb-1', 'runbook', 'T-1'),
    row('d-old', 'readiness', 'T-1'),
    row('r-other', 'record', ''),
  ]

  it('lists only readiness, record and compare reports', () => {
    const ids = runbookCandidates(entries, null).map((candidate) => candidate.entry.id)
    expect(ids).toEqual(['c-new', 'r-new', 'd-old', 'r-other'])
  })

  it('suggests reports with the first pick’s ticket first, ignoring case and spaces', () => {
    const first = entries.find((candidate) => candidate.id === 'd-old')
    const rows = runbookCandidates(entries, first ?? null)
    expect(rows.map((candidate) => candidate.entry.id)).toEqual([
      'r-new',
      'd-old',
      'c-new',
      'r-other',
    ])
    expect(rows.map((candidate) => candidate.suggested)).toEqual([true, true, false, false])
  })

  it('suggests nothing when the first pick has no ticket', () => {
    const first = entries.find((candidate) => candidate.id === 'r-other')
    expect(runbookCandidates(entries, first ?? null).some((candidate) => candidate.suggested)).toBe(
      false,
    )
  })

  it('says why a pick cannot be a runbook', () => {
    expect(runbookProblem([])).toBe('Pick at least one report.')
    expect(runbookProblem([{ kind: 'readiness' }, { kind: 'readiness' }])).toBe(
      'A runbook has one readiness report.',
    )
    expect(runbookProblem([{ kind: 'readiness' }, { kind: 'record' }, { kind: 'record' }])).toBe('')
    expect(runbookProblem(Array.from({ length: 25 }, () => ({ kind: 'record' as const })))).toMatch(
      /at most 24/,
    )
  })

  it('takes its job from the first report and sends ids only', async () => {
    const first = row('d-old', 'readiness', 'T-1')
    expect(runbookJob(first)).toEqual(first.job)
    expect(runbookJob(undefined).ticket).toBe('')
    core.invoke.mockResolvedValueOnce({ ...first, id: 'runbook-1', kind: 'runbook' })
    await createRunbook(first.job, [first, row('r-new', 'record', 'T-1')])
    expect(core.invoke).toHaveBeenLastCalledWith('runbook_create', {
      job: first.job,
      parts: [
        { kind: 'readiness', id: 'd-old' },
        { kind: 'record', id: 'r-new' },
      ],
    })
  })
})
