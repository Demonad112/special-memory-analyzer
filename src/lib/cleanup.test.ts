// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, type Component } from 'vue'
import DuplicatesPanel from '@/components/DuplicatesPanel.vue'
import JunkPanel from '@/components/JunkPanel.vue'
import QuickWins from '@/components/QuickWins.vue'
import StalePanel from '@/components/StalePanel.vue'

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
  escapeCancelsScan,
  nextTabIndex,
  groupsIn,
  offeredItems,
  openFolder,
  openTrail,
  removeItem,
  rowsByNode,
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
  loadLastRun,
  removeMany,
  restoreLast,
  restoreSentence,
  rowsFor,
  setSizeMode,
  shownBytes,
  sizeMode,
  summarize,
  toggleTier,
  dupeProgressLine,
  lastAccessNote,
  loadStale,
  type StaleReport,
  findDuplicates,
  groupsWithoutCopy,
  tickedDisk,
  type DupeGroup,
  type ItemResult,
  type RestoreResult,
  type JunkGroup,
  type JunkItem,
  type JunkKind,
  type QuickWin,
  type CleanupRow,
  type Drive,
  type Overview,
  type Tier,
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

  it('keeps the breadcrumb and list together when two opens overlap', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100)]
    cleanupJob.rows = []
    let releaseFirst: (rows: CleanupRow[]) => void = () => undefined
    core.invoke
      .mockImplementationOnce(
        () =>
          new Promise<CleanupRow[]>((resolve) => {
            releaseFirst = resolve
          }),
      )
      .mockResolvedValueOnce([row(5, 'Docs', 10)])
    const first = openFolder(row(1, 'Users', 100))
    await openFolder(row(2, 'Windows', 50))
    releaseFirst([row(3, 'Sam', 60)])
    await first

    expect(cleanupJob.trail.map((folder) => folder.name)).toEqual(['C:\\', 'Windows'])
    expect(cleanupJob.rows.map((item) => item.name)).toEqual(['Docs'])
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

describe('escapeCancelsScan', () => {
  it('cancels only a running scan with nothing open on top of it', () => {
    expect(escapeCancelsScan('Escape', true, false)).toBe(true)
    expect(escapeCancelsScan('Escape', true, true)).toBe(false)
    expect(escapeCancelsScan('Escape', false, false)).toBe(false)
    expect(escapeCancelsScan('Enter', true, false)).toBe(false)
  })
})

describe('nextTabIndex', () => {
  it('moves with the arrow keys, wrapping, and jumps with Home and End', () => {
    expect(nextTabIndex('ArrowRight', 0, 2)).toBe(1)
    expect(nextTabIndex('ArrowRight', 1, 2)).toBe(0)
    expect(nextTabIndex('ArrowLeft', 0, 2)).toBe(1)
    expect(nextTabIndex('Home', 1, 2)).toBe(0)
    expect(nextTabIndex('End', 0, 2)).toBe(1)
    expect(nextTabIndex('a', 0, 2)).toBeNull()
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

  it('hands the confirmation token from the dry run to the delete', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100)]
    core.invoke
      .mockResolvedValueOnce({ results: [result(2, 'done', 60)], overview: overview(40) })
      .mockResolvedValueOnce([])

    await removeMany([2], false, 'tok')

    expect(core.invoke).toHaveBeenNthCalledWith(1, 'cleanup_delete_many', {
      ids: [2],
      permanent: false,
      confirmToken: 'tok',
    })
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

describe('suggested cleanups by tier', () => {
  function junk(id: number, tier: Tier, overrides: Partial<JunkItem> = {}): JunkItem {
    return {
      id,
      ids: [id],
      name: `item ${String(id)}`,
      path: `C:\\junk\\${String(id)}`,
      size: 10,
      files: 1,
      isFolder: true,
      modifiedMs: null,
      cloudBytes: 0,
      tier,
      confidence: 'regenerates',
      why: 'Rebuilt on demand.',
      restore: null,
      ...overrides,
    }
  }

  function group(kind: JunkKind, tier: Tier, items: JunkItem[]): JunkGroup {
    return {
      kind,
      tier,
      confidence: 'regenerates',
      title: kind,
      description: '',
      why: 'Why suggested.',
      size: items.reduce((sum, item) => sum + item.size, 0),
      count: items.length,
      cloudBytes: 0,
      items,
    }
  }

  const safeRows = [junk(1, 'safe'), junk(2, 'safe')]
  const reviewRow = junk(3, 'review', {
    confidence: 'user_data',
    restore: 'Re-download from the vendor.',
  })
  const neverRow = junk(4, 'never', { ids: [] })
  const groups: JunkGroup[] = [
    group('tempFiles', 'safe', safeRows),
    group('installers', 'review', [reviewRow]),
    group('unidentified', 'never', [neverRow]),
  ]

  it('keeps the groups of each tier together, in the order they came', () => {
    expect(groupsIn(groups, 'safe').map((g) => g.kind)).toEqual(['tempFiles'])
    expect(groupsIn(groups, 'review').map((g) => g.kind)).toEqual(['installers'])
    expect(groupsIn(groups, 'never').map((g) => g.kind)).toEqual(['unidentified'])
  })

  it('offers only rows that have something to delete', () => {
    expect(offeredItems(groups, 'safe').map((item) => item.id)).toEqual([1, 2])
    expect(offeredItems(groups, 'never')).toEqual([])
  })

  it('select all ticks only the rows in its own tier', () => {
    expect([...toggleTier(groups, 'safe', new Set())]).toEqual([1, 2])
    expect([...toggleTier(groups, 'safe', new Set([3]))].sort()).toEqual([1, 2, 3])
  })

  it('select all clears its tier once every row there is ticked', () => {
    expect([...toggleTier(groups, 'safe', new Set([1, 2, 3]))]).toEqual([3])
  })

  it('maps every node a collapsed row stands for back to that row', () => {
    const folders = junk(5, 'safe', { ids: [5, 6, 7] })
    const map = rowsByNode([folders])
    expect(map.get(6)).toBe(folders)
    expect(map.get(7)).toBe(folders)
    expect(map.has(8)).toBe(false)
  })
})

describe('suggested cleanups on screen', () => {
  const junkGroups: JunkGroup[] = [
    {
      kind: 'tempFiles',
      tier: 'safe',
      confidence: 'regenerates',
      title: 'Temp files',
      description: 'Scratch files.',
      why: 'Temp folder contents.',
      size: 10,
      count: 1,
      cloudBytes: 0,
      items: [
        {
          id: 1,
          ids: [1],
          name: 'Temp',
          path: 'C:\\Temp',
          size: 10,
          files: 1,
          isFolder: true,
          modifiedMs: null,
          cloudBytes: 0,
          tier: 'safe',
          confidence: 'regenerates',
          why: 'Temp folder contents.',
          restore: null,
        },
      ],
    },
    {
      kind: 'installers',
      tier: 'review',
      confidence: 'user_data',
      title: 'Old installers',
      description: 'Setup files.',
      why: 'Older than 90 days.',
      size: 20,
      count: 1,
      cloudBytes: 0,
      items: [
        {
          id: 2,
          ids: [2],
          name: 'setup.exe',
          path: 'C:\\Downloads\\setup.exe',
          size: 20,
          files: 1,
          isFolder: false,
          modifiedMs: null,
          cloudBytes: 0,
          tier: 'review',
          confidence: 'user_data',
          why: 'Older than 90 days.',
          restore: 'Download it again from the vendor.',
        },
      ],
    },
    {
      kind: 'unidentified',
      tier: 'never',
      confidence: 'user_data',
      title: 'Unidentified',
      description: 'Reported only.',
      why: 'Not recognised.',
      size: 30,
      count: 1,
      cloudBytes: 0,
      items: [
        {
          id: 3,
          ids: [],
          name: 'Stuff',
          path: 'C:\\Stuff',
          size: 30,
          files: 1,
          isFolder: true,
          modifiedMs: null,
          cloudBytes: 0,
          tier: 'never',
          confidence: 'user_data',
          why: 'Not recognised.',
          restore: null,
        },
      ],
    },
  ]

  async function mount(component: Component): Promise<HTMLElement> {
    const host = document.createElement('div')
    document.body.append(host)
    createApp(component).mount(host)
    await new Promise((resolve) => setTimeout(resolve, 0))
    return host
  }

  afterEach(() => {
    core.invoke.mockReset()
    document.body.innerHTML = ''
  })

  it('shows Safe, Review and Never listed in that order with nothing ticked', async () => {
    core.invoke.mockImplementation((command: string) =>
      command === 'cleanup_junk' ? Promise.resolve(junkGroups) : Promise.reject(new Error(command)),
    )
    const host = await mount(JunkPanel)

    const headings = [...host.querySelectorAll('h3')].map((h) => h.textContent?.trim())
    expect(headings).toEqual(['Safe', 'Review', 'Never listed'])
    expect(host.querySelectorAll('input[type="checkbox"]:checked')).toHaveLength(0)
    const review = host.querySelector('section[aria-labelledby="junk-review"]')
    expect(review?.textContent).toContain('Restore: Download it again from the vendor.')
    const never = host.querySelector('section[aria-labelledby="junk-never"]')
    expect(never?.querySelector('input[type="checkbox"]')).toBeNull()
  })

  it('offers select all only in the Safe section', async () => {
    core.invoke.mockImplementation((command: string) =>
      command === 'cleanup_junk' ? Promise.resolve(junkGroups) : Promise.reject(new Error(command)),
    )
    const host = await mount(JunkPanel)

    expect(host.querySelector('section[aria-labelledby="junk-safe"]')?.textContent).toContain(
      'Select all',
    )
    expect(host.querySelector('section[aria-labelledby="junk-review"]')?.textContent).not.toContain(
      'Select all',
    )
    expect(host.querySelector('section[aria-labelledby="junk-never"]')?.textContent).not.toContain(
      'Select all',
    )
  })

  it('ticks nothing in the quick cleanups, caches included', async () => {
    core.invoke.mockImplementation((command: string) =>
      command === 'cleanup_quick_list'
        ? Promise.resolve({
            elevated: true,
            items: [
              {
                id: 'userTemp',
                title: 'User temporary files',
                description: '',
                needsAdmin: false,
                size: 10,
                files: 1,
              },
              {
                id: 'recycleBin',
                title: 'Recycle Bin',
                description: '',
                needsAdmin: false,
                size: 5,
                files: 1,
              },
            ],
          })
        : Promise.reject(new Error(command)),
    )
    const host = await mount(QuickWins)

    expect(host.querySelectorAll('input[type="checkbox"]')).toHaveLength(2)
    expect(host.querySelectorAll('input[type="checkbox"]:checked')).toHaveLength(0)
  })
})

describe('restore the last cleanup', () => {
  beforeEach(() => core.invoke.mockReset())

  it('reads the last run and restores it through the matching commands', async () => {
    core.invoke.mockResolvedValueOnce({ time: 't', items: 3, recycled: 2 })
    expect(await loadLastRun()).toEqual({ time: 't', items: 3, recycled: 2 })
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_last_run')

    core.invoke.mockResolvedValueOnce([])
    await restoreLast()
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_restore_last')
  })

  it('says how many came back and how many did not', () => {
    const item = (outcome: RestoreResult['outcome']): RestoreResult => ({
      path: 'C:\\x',
      size: 1,
      outcome,
      message: null,
    })
    expect(restoreSentence([item('restored'), item('restored')])).toBe('2 of 2 restored.')
    expect(restoreSentence([item('restored'), item('exists'), item('notInBin')])).toBe(
      '1 of 3 restored; 2 could not be put back.',
    )
  })
})

describe('duplicates', () => {
  const group = (id: number, ids: number[]): DupeGroup => ({
    id,
    size: 100,
    reclaimable: 100 * (ids.length - 1),
    members: ids.map((member) => ({
      id: member,
      name: 'setup.iso',
      folder: `D:\\copy${String(member)}`,
      modifiedMs: 1_600_000_000_000 + member,
      disk: 100,
    })),
  })

  afterEach(() => {
    core.invoke.mockReset()
    document.body.innerHTML = ''
  })

  it('flags a group whose every copy is ticked', () => {
    const groups = [group(0, [1, 2, 3]), group(1, [4, 5])]
    expect(groupsWithoutCopy(groups, new Set([2, 3, 5]))).toEqual([])
    expect(groupsWithoutCopy(groups, new Set([1, 2, 3, 5])).map((g) => g.id)).toEqual([0])
    expect(tickedDisk(groups, new Set([2, 3, 5]))).toBe(300)
  })

  it('describes each stage of the search', () => {
    expect(dupeProgressLine(null)).toBe('Starting…')
    expect(dupeProgressLine({ stage: 'ends', done: 3, total: 10, bytes: 0 })).toBe(
      'Comparing the start and end of each file: 3 of 10…',
    )
    expect(dupeProgressLine({ stage: 'sizes', done: 0, total: 0, bytes: 0 })).toBe(
      'Listing files of the same size…',
    )
  })

  it('streams progress from the search and passes the minimum size', async () => {
    const seen: string[] = []
    core.invoke.mockImplementationOnce(() => {
      core.channels[0]?.onmessage({ stage: 'full', done: 1, total: 2, bytes: 10 })
      return Promise.resolve({ minSize: 1048576, groups: 0, files: 0, reclaimable: 0 })
    })
    await findDuplicates(1048576, (progress) => seen.push(progress.stage))
    expect(core.invoke).toHaveBeenCalledWith('cleanup_dupes_find', {
      minSize: 1048576,
      onProgress: core.channels[0],
    })
    expect(seen).toEqual(['full'])
  })

  it('deletes duplicates through their own command', async () => {
    cleanupJob.trail = [row(0, 'C:\\', 100)]
    core.invoke
      .mockResolvedValueOnce({ results: [], overview: overview(40) })
      .mockResolvedValueOnce([])
    await removeMany([2, 3], false, undefined, 'cleanup_delete_duplicates')
    expect(core.invoke).toHaveBeenNthCalledWith(1, 'cleanup_delete_duplicates', {
      ids: [2, 3],
      permanent: false,
    })
    expect(core.invoke).not.toHaveBeenCalledWith('cleanup_delete_many', expect.anything())
  })

  it('ticks nothing until the button is pressed, then all but the oldest', async () => {
    const groups = [group(0, [1, 2, 3])]
    core.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'cleanup_dupes_find':
          return Promise.resolve({
            minSize: 1048576,
            groups: 1,
            files: 3,
            reclaimable: 200,
            skipped: { cloud: 2, hardLinked: 0, unreadable: 0 },
          })
        case 'cleanup_dupes_groups':
          return Promise.resolve(groups)
        case 'cleanup_dupes_all_but_oldest':
          return Promise.resolve([2, 3])
        default:
          return Promise.reject(new Error(command))
      }
    })
    const host = document.createElement('div')
    document.body.append(host)
    createApp(DuplicatesPanel).mount(host)
    const button = (label: string): HTMLButtonElement =>
      [...host.querySelectorAll('button')].find((b) => b.textContent?.includes(label))!
    const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0))

    button('Find duplicates').click()
    await settle()
    expect(host.textContent).toContain('1 group, 3 files')
    expect(host.textContent).toContain('2 online-only files (never downloaded)')
    expect(host.querySelectorAll('input[type="checkbox"]')).toHaveLength(3)
    expect(host.querySelectorAll('input[type="checkbox"]:checked')).toHaveLength(0)

    button('Select all but the oldest').click()
    await settle()
    const boxes = [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')]
    expect(boxes.map((box) => box.checked)).toEqual([false, true, true])
    expect(button('Review').disabled).toBe(false)

    boxes[0]?.click()
    await settle()
    expect(host.textContent).toContain('Keep at least one copy of each.')
    expect(button('Review').disabled).toBe(true)
  })
})

describe('stale data', () => {
  const report: StaleReport = {
    years: 3,
    notOpened: false,
    nowMs: 1_792_800_000_000,
    groups: [
      {
        id: 4,
        name: 'Finance',
        isFolder: true,
        staleFiles: 2,
        staleBytes: 600,
        folderBytes: 1000,
        newestMs: 1_600_000_000_000,
      },
    ],
    staleFiles: 2,
    staleBytes: 600,
    unknownFiles: 1,
    unknownBytes: 10,
    cloudFiles: 0,
    totalBytes: 2000,
    lastAccess: 'off',
  }

  afterEach(() => {
    core.invoke.mockReset()
    document.body.innerHTML = ''
  })

  it('asks Rust with the threshold and the access filter', async () => {
    core.invoke.mockResolvedValueOnce(report)
    await loadStale(5, true)
    expect(core.invoke).toHaveBeenLastCalledWith('cleanup_stale', { years: 5, notOpened: true })
  })

  it('warns when Windows is not keeping access times', () => {
    expect(lastAccessNote('off')).toContain('NtfsDisableLastAccessUpdate')
    expect(lastAccessNote('unknown')).toContain('hint')
    expect(lastAccessNote('kept')).toContain('keeps')
  })

  it("defaults to 3 years by modified time and lists a folder's files only when opened", async () => {
    core.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'cleanup_stale':
          return Promise.resolve(report)
        case 'cleanup_stale_files':
          return Promise.resolve([
            { id: 9, path: 'Finance\\a.xlsx', size: 400, modifiedMs: 1, accessedMs: null },
          ])
        default:
          return Promise.reject(new Error(command))
      }
    })
    const host = document.createElement('div')
    document.body.append(host)
    createApp(StalePanel).mount(host)
    const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0))
    await settle()

    expect(core.invoke).toHaveBeenCalledWith('cleanup_stale', { years: 3, notOpened: false })
    expect(host.textContent).toContain('600 bytes in 2 files (30% of the scan)')
    expect(host.textContent).toContain('Not counted: 1 file (10 bytes) with the date unknown')
    expect(host.querySelectorAll('input[type="checkbox"]')).toHaveLength(1)
    expect(host.querySelector('input[type="checkbox"]:checked')).toBeNull()

    const row = [...host.querySelectorAll('button')].find((b) => b.textContent?.includes('Finance'))
    row?.click()
    await settle()
    expect(core.invoke).toHaveBeenCalledWith('cleanup_stale_files', {
      group: 4,
      years: 3,
      notOpened: false,
    })
    const boxes = host.querySelectorAll('input[type="checkbox"]')
    expect(boxes).toHaveLength(2)
    expect(host.querySelectorAll('input[type="checkbox"]:checked')).toHaveLength(0)
  })
})
