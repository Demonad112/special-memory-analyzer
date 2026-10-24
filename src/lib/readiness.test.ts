// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => true),
  channels: [] as { onmessage: (message: unknown) => void }[],
}))

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

import {
  CANCELLED,
  displayName,
  groupCounts,
  loadForm,
  readinessJob,
  runReadinessJob,
  saveForm,
  setFilter,
  startReadiness,
  toggleRow,
  visibleRows,
  type ReadinessResult,
  type ReadinessRow,
} from './readiness'

function row(id: number, name: string, hasChildren = false): ReadinessRow {
  return { id, name, kind: hasChildren ? 'dir' : 'file', problems: 1, own: [], hasChildren }
}

function result(rows: ReadinessRow[] = []): ReadinessResult {
  return {
    report: {
      source: 'F:\\Finance',
      destinationRoot: '',
      target: 'fileServer',
      files: 1,
      dirs: 0,
      size: 0,
      cloudFiles: 0,
      links: 0,
      unreadable: 0,
      nameProblems: 1,
      longPaths: 0,
      blockers: 1,
      problemsTotal: 1,
      elapsedMs: 5,
      counts: [],
    },
    root: row(0, 'F:\\Finance', true),
    rows,
    reportId: 'readiness-1',
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  core.channels.length = 0
  localStorage.clear()
  Object.assign(readinessJob, {
    running: false,
    result: null,
    filter: null,
    rows: [],
    children: new Map(),
    expanded: new Set(),
    loading: new Set(),
  })
})

describe('readiness commands', () => {
  it('starts a check with the job and forwards progress', async () => {
    core.invoke.mockResolvedValue(result())
    const onProgress = vi.fn()
    await startReadiness('F:\\Finance', 'D:\\Shares', 'sharePoint', onProgress)
    expect(core.invoke).toHaveBeenCalledWith('readiness_start', {
      path: 'F:\\Finance',
      destination: 'D:\\Shares',
      target: 'sharePoint',
      job: { client: '', ticket: '', technician: '' },
      onEvent: core.channels[0],
    })
    const progress = { phase: 'names', scan: { files: 3, dirs: 1, bytes: 9, current: 'x' } }
    core.channels[0]?.onmessage({ kind: 'progress', progress })
    expect(onProgress).toHaveBeenCalledWith(progress)
  })
})

describe('displayName', () => {
  it('quotes a name that starts or ends with a space', () => {
    expect(displayName('notes ')).toBe('"notes "')
    expect(displayName(' a')).toBe('" a"')
    expect(displayName('a b.txt')).toBe('a b.txt')
  })
})

describe('groupCounts', () => {
  it('adds up rules per group in screen order and leaves out empty groups', () => {
    expect(
      groupCounts({
        counts: [
          { kind: 'link', rule: 'link', count: 2, blocker: false },
          { kind: 'name', rule: 'reserved', count: 3, blocker: true },
          { kind: 'name', rule: 'sharePointOfficeLock', count: 1, blocker: false },
        ],
      }),
    ).toEqual([
      { kind: 'name', heading: 'Names', count: 4, blockers: 3 },
      { kind: 'link', heading: 'Links (not followed)', count: 2, blockers: 0 },
    ])
  })
})

describe('visibleRows', () => {
  it('shows open folders’ children under them, in order', () => {
    const top = [row(1, 'a', true), row(2, 'b')]
    const children = new Map([
      [1, [row(3, 'a1', true), row(4, 'a2')]],
      [3, [row(5, 'deep')]],
    ])
    const names = (expanded: number[]): string[] =>
      visibleRows(top, children, new Set(expanded)).map(
        (item) => `${'-'.repeat(item.depth)}${item.row.name}`,
      )
    expect(names([])).toEqual(['a', 'b'])
    expect(names([1])).toEqual(['a', '-a1', '-a2', 'b'])
    expect(names([1, 3])).toEqual(['a', '-a1', '--deep', '-a2', 'b'])
    // A closed parent hides an open child.
    expect(names([3])).toEqual(['a', 'b'])
  })
})

describe('the remembered form', () => {
  it('round-trips and falls back to the file server profile', () => {
    expect(loadForm()).toEqual({ path: '', destination: '', target: 'fileServer' })
    saveForm({ path: 'F:\\x', destination: 'D:\\y', target: 'sharePoint' })
    expect(loadForm()).toEqual({ path: 'F:\\x', destination: 'D:\\y', target: 'sharePoint' })
    localStorage.setItem('analyzer-readiness', '{"path":3,"target":"cloud"}')
    expect(loadForm()).toEqual({ path: '', destination: '', target: 'fileServer' })
  })
})

describe('readinessJob', () => {
  const form = { path: 'F:\\x', destination: '', target: 'fileServer' } as const

  it('keeps the result and its top rows', async () => {
    core.invoke.mockResolvedValue(result([row(1, 'a')]))
    await runReadinessJob(form)
    expect(readinessJob.running).toBe(false)
    expect(readinessJob.rows.map((item) => item.name)).toEqual(['a'])
    expect(readinessJob.root?.id).toBe(0)
  })

  it('stays quiet on a cancel and shows other errors', async () => {
    core.invoke.mockRejectedValueOnce(CANCELLED)
    await runReadinessJob(form)
    expect(readinessJob.error).toBe('')
    expect(readinessJob.notice).toContain('cancelled')
    core.invoke.mockRejectedValueOnce('F:\\x does not exist.')
    await runReadinessJob(form)
    expect(readinessJob.error).toBe('F:\\x does not exist.')
  })

  it('drops children that arrive after the filter changed', async () => {
    core.invoke.mockResolvedValue(result([row(1, 'a', true)]))
    await runReadinessJob(form)
    let release: (rows: ReadinessRow[]) => void = () => undefined
    core.invoke.mockReturnValueOnce(
      new Promise((resolve) => {
        release = resolve
      }),
    )
    const pending = toggleRow(1)
    core.invoke.mockResolvedValueOnce(row(0, 'root')).mockResolvedValueOnce([])
    await setFilter('longPath')
    release([row(9, 'late')])
    await pending
    expect(readinessJob.children.has(1)).toBe(false)
    expect(readinessJob.filter).toBe('longPath')
    expect(core.invoke).toHaveBeenCalledWith('readiness_children', { id: 0, kind: 'longPath' })
  })

  it('loads children once with the current filter', async () => {
    core.invoke.mockResolvedValue(result([row(1, 'a', true)]))
    await runReadinessJob(form)
    core.invoke.mockResolvedValueOnce([row(2, 'b')])
    await toggleRow(1)
    expect(core.invoke).toHaveBeenLastCalledWith('readiness_children', { id: 1, kind: null })
    await toggleRow(1)
    await toggleRow(1)
    expect(readinessJob.children.get(1)?.map((item) => item.name)).toEqual(['b'])
    expect(readinessJob.expanded.has(1)).toBe(true)
  })
})
