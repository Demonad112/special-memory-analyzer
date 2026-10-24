// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => true),
  channels: [] as { onmessage: (message: unknown) => void }[],
}))
const dialog = vi.hoisted(() => ({ open: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  isTauri: core.isTauri,
  Channel: class {
    onmessage: (message: unknown) => void = () => undefined
    constructor() {
      core.channels.push(this)
    }
  },
}))
vi.mock('@tauri-apps/plugin-dialog', () => dialog)

import {
  cancelCompare,
  cleanPath,
  compareJob,
  runCompareJob,
  toggleRow,
  formatMissingList,
  isCancelled,
  loadChildren,
  loadPaths,
  loadRecent,
  missingDelta,
  pickFolder,
  resetMissingDelta,
  rememberRecent,
  revealRow,
  rowPath,
  savePaths,
  startCompare,
  type CompareResult,
  type DiffSummary,
} from './compare'

beforeEach(() => {
  vi.clearAllMocks()
  core.isTauri.mockReturnValue(true)
  core.channels.length = 0
  localStorage.clear()
})

describe('compare commands', () => {
  it('starts a compare and forwards progress events', async () => {
    core.invoke.mockResolvedValue({ rows: [] })
    const onProgress = vi.fn()
    await startCompare('C:\\a', 'D:\\b', true, onProgress)

    expect(core.invoke).toHaveBeenCalledWith('compare_start', {
      left: 'C:\\a',
      right: 'D:\\b',
      ignoreJunk: true,
      job: { client: '', ticket: '', technician: '' },
      onEvent: core.channels[0],
    })
    const progress = { files: 1, dirs: 1, bytes: 5, current: 'C:\\a' }
    core.channels[0]?.onmessage({
      kind: 'progress',
      left: progress,
      right: progress,
      leftDone: true,
      rightDone: false,
    })
    expect(onProgress).toHaveBeenCalledWith({
      left: progress,
      right: progress,
      leftDone: true,
      rightDone: false,
    })
  })

  it('loads children and cancels', async () => {
    core.invoke.mockResolvedValue([])
    await loadChildren(7)
    expect(core.invoke).toHaveBeenCalledWith('compare_children', { id: 7 })
    await cancelCompare()
    expect(core.invoke).toHaveBeenCalledWith('compare_cancel')
    await rowPath(3, 'right')
    expect(core.invoke).toHaveBeenCalledWith('compare_path', { id: 3, side: 'right' })
    await revealRow(3, 'left')
    expect(core.invoke).toHaveBeenCalledWith('compare_reveal', { id: 3, side: 'left' })
  })

  it('formats the missing list for pasting', () => {
    const summary = { missing: 2, missingBytes: 2048 } as DiffSummary
    const text = formatMissingList(
      { source: 'C:\\a', destination: 'D:\\b', paths: ['x.txt', 'sub\\y.txt'] },
      summary,
      new Date(2026, 9, 3, 15, 42),
    )
    expect(text.split('\r\n')).toEqual([
      'Missing at destination: 2 files (2.00 KB)',
      'Source: C:\\a',
      'Destination: D:\\b',
      'Compared: Oct 3, 2026, 3:42 PM',
      '',
      'x.txt',
      'sub\\y.txt',
    ])
  })

  it('cleans pasted paths', () => {
    expect(cleanPath('  "C:\\My Data"  ')).toBe('C:\\My Data')
    expect(cleanPath('D:\\x ')).toBe('D:\\x')
    expect(cleanPath('"')).toBe('"')
  })

  it('recognises the cancel error and other messages', () => {
    expect(isCancelled('cancelled')).toBe(true)
    expect(isCancelled('Source: nope')).toBe(false)
  })
})

describe('pickFolder', () => {
  it('returns the picked folder', async () => {
    dialog.open.mockResolvedValue('C:\\Data')
    await expect(pickFolder('Source')).resolves.toBe('C:\\Data')
    expect(dialog.open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false }),
    )
  })

  it('returns null when cancelled or outside the app', async () => {
    dialog.open.mockResolvedValue(null)
    await expect(pickFolder('Source')).resolves.toBeNull()
    core.isTauri.mockReturnValue(false)
    await expect(pickFolder('Source')).resolves.toBeNull()
    expect(dialog.open).toHaveBeenCalledTimes(1)
  })
})

describe('remembered paths', () => {
  it('round-trips through localStorage', () => {
    expect(loadPaths()).toEqual({ left: '', right: '' })
    savePaths({ left: 'C:\\a', right: 'D:\\b' })
    expect(loadPaths()).toEqual({ left: 'C:\\a', right: 'D:\\b' })
  })

  it('ignores corrupt values', () => {
    localStorage.setItem('analyzer-compare-paths', '{nope')
    expect(loadPaths()).toEqual({ left: '', right: '' })
    localStorage.setItem('analyzer-compare-paths', JSON.stringify({ left: 3, right: 'x' }))
    expect(loadPaths()).toEqual({ left: '', right: 'x' })
  })
})

describe('recent compares', () => {
  it('keeps the newest five, without duplicates', () => {
    for (let i = 0; i < 6; i++) {
      rememberRecent({ left: `C:\\${String(i)}`, right: 'D:\\x' })
    }
    rememberRecent({ left: 'c:\\3', right: 'd:\\X' })
    const recent = loadRecent()
    expect(recent).toHaveLength(5)
    expect(recent[0]).toEqual({ left: 'c:\\3', right: 'd:\\X' })
    expect(recent.filter((pair) => pair.left.toLowerCase() === 'c:\\3')).toHaveLength(1)
  })

  it('ignores corrupt values', () => {
    localStorage.setItem('analyzer-compare-recent', '{"a":1}')
    expect(loadRecent()).toEqual([])
  })
})

describe('missingDelta', () => {
  const pair = { left: 'C:\\A', right: 'D:\\B' }

  beforeEach(() => {
    resetMissingDelta()
  })

  it('says nothing for the first compare of a pair', () => {
    expect(missingDelta(pair, 612)).toBeNull()
  })

  it('shows before and after on a re-check', () => {
    missingDelta(pair, 612)
    expect(missingDelta({ left: 'c:\\a', right: 'd:\\b' }, 4)).toBe('612 → 4 still missing')
  })

  it('reports an all-clear and an unchanged count', () => {
    missingDelta(pair, 3)
    expect(missingDelta(pair, 3)).toBe('3 still missing, no change since the last compare')
    expect(missingDelta(pair, 0)).toBe('All 3 missing files are now at the destination')
    expect(missingDelta(pair, 0)).toBeNull()
  })

  it('starts over for a different pair', () => {
    missingDelta(pair, 9)
    expect(missingDelta({ left: 'C:\\A', right: 'E:\\Other' }, 2)).toBeNull()
  })
})

describe('compareJob', () => {
  const finished = {
    rows: [],
    summary: { missing: 0 },
    elapsedMs: 1,
    reportId: null,
  } as unknown as CompareResult

  function reset(): void {
    Object.assign(compareJob, {
      running: false,
      result: null,
      error: '',
      notice: '',
      children: new Map(),
      expanded: new Set(),
      loading: new Set(),
    })
  }

  it('keeps the result in shared state and refuses a second run while busy', async () => {
    reset()
    let finish: (value: CompareResult) => void = () => undefined
    core.invoke.mockImplementationOnce(
      () =>
        new Promise<CompareResult>((resolve) => {
          finish = resolve
        }),
    )
    const first = runCompareJob('C:\a', 'D:\b', true)
    expect(compareJob.running).toBe(true)

    await runCompareJob('C:\a', 'D:\b', true)
    expect(core.invoke).toHaveBeenCalledTimes(1)

    finish(finished)
    await first
    expect(compareJob.running).toBe(false)
    expect(compareJob.result).toBe(finished)
    expect(compareJob.compared).toEqual({ left: 'C:\a', right: 'D:\b' })
  })

  it('drops children that arrive after a newer compare replaced the result', async () => {
    reset()
    compareJob.result = finished
    let release: (rows: never[]) => void = () => undefined
    core.invoke.mockImplementationOnce(
      () =>
        new Promise<never[]>((resolve) => {
          release = resolve
        }),
    )
    const pending = toggleRow(4)
    compareJob.result = { ...finished }
    compareJob.children = new Map()
    release([])
    await pending

    expect(compareJob.children.has(4)).toBe(false)
  })
})
