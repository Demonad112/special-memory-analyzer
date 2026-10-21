import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({
  invoke: vi.fn(),
  channels: [] as { onmessage: (message: unknown) => void }[],
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  isTauri: () => true,
  Channel: class {
    onmessage: (message: unknown) => void = () => undefined
    constructor() {
      core.channels.push(this)
    }
  },
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

import {
  cleanupJob,
  driveFor,
  openFolder,
  openTrail,
  removeItem,
  runScan,
  scanPercent,
  canClean,
  runQuickClean,
  appAdmin,
  canDelete,
  deleteSentence,
  loadJunk,
  needsAdmin,
  outcomeLabel,
  previewDelete,
  refreshElevated,
  removeMany,
  rowsFor,
  setSizeMode,
  shownBytes,
  sizeMode,
  summarize,
  type ItemResult,
  type QuickWin,
  type CleanupRow,
  type Drive,
  type Overview,
} from './cleanup'

function row(id: number, name: string, size: number): CleanupRow {
  return {
    id,
    name,
    kind: 'dir',
    size,
    logical: size,
    hardLinked: false,
    files: 1,
    dirs: 0,
    cloudFiles: 0,
    cloudBytes: 0,
    errors: 0,
    error: null,
    modifiedMs: null,
    hasChildren: true,
  }
}

function overview(size: number): Overview {
  return { root: row(0, 'C:\\', size), largestFiles: [], fileTypes: [] }
}

const drive: Drive = { path: 'C:\\', label: 'Local Disk', total: 1000, free: 600, removable: false }

beforeEach(() => {
  vi.clearAllMocks()
  core.channels.length = 0
})

describe('cleanup scan', () => {
  it('streams progress and opens the scanned root', async () => {
    core.invoke.mockImplementationOnce(() => {
      core.channels[0]?.onmessage({ files: 3, dirs: 1, bytes: 50, current: 'C:\\x' })
      expect(cleanupJob.running).toBe(true)
      expect(cleanupJob.progress?.bytes).toBe(50)
      return Promise.resolve({
        path: 'C:\\',
        overview: overview(100),
        rows: [row(1, 'Users', 100)],
        elapsedMs: 5,
      })
    })
    await runScan('C:\\')

    expect(core.invoke).toHaveBeenCalledWith('cleanup_scan', {
      job: { client: '', ticket: '', technician: '' },
      path: 'C:\\',
      onProgress: core.channels[0],
    })
    expect(cleanupJob.running).toBe(false)
    expect(cleanupJob.trail.map((folder) => folder.id)).toEqual([0])
    expect(cleanupJob.rows.map((item) => item.name)).toEqual(['Users'])
  })

  it('stays quiet on cancel and shows other errors', async () => {
    core.invoke.mockRejectedValueOnce('cancelled')
    await runScan('C:\\')
    expect(cleanupJob.notice).toBe('Scan cancelled.')
    expect(cleanupJob.error).toBe('')

    core.invoke.mockRejectedValueOnce('D:\\ does not exist.')
    await runScan('D:\\')
    expect(cleanupJob.error).toBe('D:\\ does not exist.')
    expect(cleanupJob.overview).toBeNull()
  })

  it('walks into folders and back up the trail', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100)]
    core.invoke.mockResolvedValueOnce([row(2, 'Sam', 60)])
    await openFolder(row(1, 'Users', 100))
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_children', { id: 1 })
    expect(cleanupJob.trail.map((folder) => folder.name)).toEqual(['C:\\', 'Users'])

    core.invoke.mockResolvedValueOnce([row(1, 'Users', 100)])
    await openTrail(0)
    expect(cleanupJob.trail).toHaveLength(1)
    expect(cleanupJob.rows[0]?.name).toBe('Users')
  })

  it('refreshes totals and the shown folder after a delete', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100), row(1, 'Users', 100)]
    core.invoke.mockResolvedValueOnce(overview(40)).mockResolvedValueOnce([row(3, 'Public', 40)])
    await removeItem(2, false)

    expect(core.invoke).toHaveBeenNthCalledWith(1, 'cleanup_delete', { id: 2, permanent: false })
    expect(core.invoke).toHaveBeenNthCalledWith(2, 'cleanup_children', { id: 1 })
    expect(cleanupJob.overview?.root.size).toBe(40)
    expect(cleanupJob.trail.map((folder) => folder.name)).toEqual(['C:\\', 'Users'])
    expect(cleanupJob.rows.map((item) => item.name)).toEqual(['Public'])
  })
})

describe('quick cleanups', () => {
  const card: QuickWin = {
    id: 'windowsTemp',
    title: 'Windows temporary files',
    description: '',
    needsAdmin: true,
    size: 10,
    files: 1,
  }

  it('can be ticked only with something to free and the rights it needs', () => {
    expect(canClean(card, true)).toBe(true)
    expect(canClean(card, false)).toBe(false)
    expect(canClean({ ...card, needsAdmin: false }, false)).toBe(true)
    expect(canClean({ ...card, size: 0 }, true)).toBe(false)
  })

  it('sends the chosen cleanups with the job details', async () => {
    core.invoke.mockResolvedValueOnce({ report: { items: [] }, reportId: 'cleanup-1' })
    await runQuickClean(['userTemp', 'recycleBin'])
    expect(core.invoke).toHaveBeenCalledWith('cleanup_quick_clean', {
      ids: ['userTemp', 'recycleBin'],
      job: { client: '', ticket: '', technician: '' },
    })
  })
})

describe('delete gating', () => {
  it('lets anyone recycle one item and asks for admin for the rest', () => {
    expect(needsAdmin(1, false)).toBe(false)
    expect(needsAdmin(2, false)).toBe(true)
    expect(needsAdmin(1, true)).toBe(true)
    expect(canDelete(1, false, false)).toBe(true)
    expect(canDelete(3, false, false)).toBe(false)
    expect(canDelete(1, true, false)).toBe(false)
    expect(canDelete(3, true, true)).toBe(true)
    expect(canDelete(0, false, true)).toBe(false)
  })

  it('says plainly where the items are going', () => {
    expect(deleteSentence(1, false)).toBe('1 item will be moved to the Recycle Bin.')
    expect(deleteSentence(4, true)).toBe(
      '4 items will be deleted permanently. This cannot be undone.',
    )
  })

  it('reads the elevation once and keeps it off when the call fails', async () => {
    core.invoke.mockResolvedValueOnce(true)
    await refreshElevated()
    expect(appAdmin.elevated).toBe(true)
    appAdmin.elevated = false
    core.invoke.mockRejectedValueOnce('nope')
    await refreshElevated()
    expect(appAdmin.elevated).toBe(false)
  })
})

describe('bulk delete calls', () => {
  const result = (id: number, outcome: ItemResult['outcome'], size: number): ItemResult => ({
    id,
    path: `C:\\x${String(id)}`,
    size,
    outcome,
    message: null,
  })

  it('asks for suggestions and previews without deleting', async () => {
    core.invoke.mockResolvedValueOnce([])
    await loadJunk()
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_junk')

    core.invoke.mockResolvedValueOnce({ items: [], goCount: 0, goSize: 0 })
    await previewDelete([4, 5])
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_delete_preview', { ids: [4, 5] })
    expect(core.invoke).not.toHaveBeenCalledWith('cleanup_delete_many', expect.anything())
  })

  it('refreshes the totals and the shown folder, and returns the per-item results', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100), row(1, 'Users', 100)]
    const results = [result(2, 'done', 60), result(3, 'inUse', 10)]
    core.invoke
      .mockResolvedValueOnce({ results, overview: overview(40) })
      .mockResolvedValueOnce([row(5, 'Public', 40)])

    const got = await removeMany([2, 3], false)

    expect(core.invoke).toHaveBeenNthCalledWith(1, 'cleanup_delete_many', {
      ids: [2, 3],
      permanent: false,
    })
    expect(core.invoke).toHaveBeenNthCalledWith(2, 'cleanup_children', { id: 1 })
    expect(got).toEqual(results)
    expect(cleanupJob.overview?.root.size).toBe(40)
    expect(cleanupJob.rows.map((item) => item.name)).toEqual(['Public'])
  })

  it('shows a notice when the deletion log could not be written', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100)]
    cleanupJob.notice = ''
    core.invoke
      .mockResolvedValueOnce({
        results: [result(2, 'done', 60)],
        overview: overview(40),
        logError: 'The deletion log could not be written: access denied',
      })
      .mockResolvedValueOnce([])

    await removeMany([2], false)

    expect(cleanupJob.notice).toBe('The deletion log could not be written: access denied')
  })

  it('counts what went and what did not', () => {
    const summary = summarize([
      result(1, 'done', 50),
      result(2, 'done', 25),
      result(3, 'inUse', 99),
      result(4, 'accessDenied', 1),
    ])
    expect(summary).toEqual({ done: 2, failed: 2, freed: 75 })
    expect(outcomeLabel('inUse')).toBe('In use')
  })
})

describe('drive helpers', () => {
  it('matches a typed path to its drive', () => {
    expect(driveFor('c:', [drive])).toBe(drive)
    expect(driveFor('C:/', [drive])).toBe(drive)
    expect(driveFor('C:\\Users', [drive])).toBeUndefined()
  })

  it('turns bytes found into a percentage of the used space', () => {
    expect(scanPercent(200, drive)).toBe(50)
    expect(scanPercent(5000, drive)).toBe(99)
    expect(scanPercent(200, undefined)).toBeNull()
  })
})

describe('size mode', () => {
  const small = { ...row(1, 'small', 8192), logical: 100 }
  const big = { ...row(2, 'big', 4096), logical: 4000 }

  it('keeps disk order and sizes in disk mode', () => {
    expect(rowsFor([small, big], 'disk')).toEqual([small, big])
    expect(shownBytes(small, 'disk')).toBe(8192)
  })

  it('swaps in logical sizes and sorts by them in logical mode', () => {
    const shown = rowsFor([small, big], 'logical')
    expect(shown.map((r) => [r.name, r.size])).toEqual([
      ['big', 4000],
      ['small', 100],
    ])
    expect(shownBytes(small, 'logical')).toBe(100)
    expect(small.size).toBe(8192)
  })

  it('remembers the choice', () => {
    setSizeMode('logical')
    expect(sizeMode.value).toBe('logical')
    expect(localStorage.getItem('analyzer-cleanup-size-mode')).toBe('logical')
    setSizeMode('disk')
    expect(sizeMode.value).toBe('disk')
  })
})
