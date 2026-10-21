import { beforeEach, describe, expect, it, vi } from 'vitest'

const core = vi.hoisted(() => ({
  invoke: vi.fn(),
  channels: [] as { onmessage: (message: unknown) => void }[],
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  Channel: class {
    onmessage: (message: unknown) => void = () => undefined
    constructor() {
      core.channels.push(this)
    }
  },
}))

import {
  bytesPerSecond,
  countByReason,
  defaultPreserve,
  formatNotCopiedList,
  formatTimeLeft,
  isCancelled,
  reasonTitle,
  retryRecord,
  startRecord,
  timeLeftMs,
  type ItemResult,
  type RecordProgress,
  type RunDetails,
} from './record'

function item(relativePath: string, reason?: ItemResult['reason']): ItemResult {
  return {
    relativePath,
    kind: 'file',
    size: 10,
    status: 'notCopied',
    reason,
    attempts: 1,
    inferred: false,
    recoverable: true,
    atMs: 0,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  core.channels.length = 0
})

describe('record commands', () => {
  it('starts a record and forwards progress from the channel', async () => {
    core.invoke.mockResolvedValue({ summary: { id: 'r1' } })
    const seen: RecordProgress[] = []

    const pending = startRecord(
      'C:\\src',
      'D:\\dst',
      'watch',
      'hash',
      {
        ignoreJunk: true,
        downloadCloud: false,
        preserve: { ...defaultPreserve, attributes: false },
      },
      (progress) => seen.push(progress),
    )
    core.channels[0]?.onmessage({ phase: 'watching', filesDone: 3 })
    await pending

    expect(core.invoke).toHaveBeenCalledWith('record_start', {
      source: 'C:\\src',
      destination: 'D:\\dst',
      mode: 'watch',
      verify: 'hash',
      options: {
        ignoreJunk: true,
        downloadCloud: false,
        preserve: { ...defaultPreserve, attributes: false },
        job: { client: '', ticket: '', technician: '' },
      },
      onEvent: core.channels[0],
    })
    expect(seen).toEqual([{ phase: 'watching', filesDone: 3 }])
  })

  it('retries by reason, by path, or everything', async () => {
    core.invoke.mockResolvedValue({})

    await retryRecord('r1', [], () => undefined, 'fileLocked')
    expect(core.invoke).toHaveBeenLastCalledWith('record_retry', {
      runId: 'r1',
      paths: [],
      reason: 'fileLocked',
      onEvent: core.channels[0],
    })
    await retryRecord('r1', ['a.txt'], () => undefined)
    expect(core.invoke).toHaveBeenLastCalledWith(
      'record_retry',
      expect.objectContaining({ paths: ['a.txt'], reason: null }),
    )
  })

  it('recognises the cancelled error', () => {
    expect(isCancelled('Cancelled')).toBe(true)
    expect(isCancelled('Access is denied')).toBe(false)
  })
})

describe('reasons', () => {
  it('has a title for every reason, and a fallback', () => {
    expect(reasonTitle('fileLocked')).toBe('In use by another program')
    expect(reasonTitle(undefined)).toBe('Other error')
  })

  it('counts not-copied rows per reason, most common first', () => {
    const counts = countByReason([
      item('a', 'missing'),
      item('b', 'accessDenied'),
      item('c', 'missing'),
      item('d'),
    ])
    expect(counts).toEqual([
      { reason: 'missing', count: 2 },
      { reason: 'accessDenied', count: 1 },
      { reason: 'unknown', count: 1 },
    ])
  })
})

describe('speed and time left', () => {
  it('waits a second before giving a speed', () => {
    expect(bytesPerSecond(1000, 500)).toBe(0)
    expect(bytesPerSecond(4000, 2000)).toBe(2000)
  })

  it('works out the time left at the average speed', () => {
    expect(timeLeftMs(3000, 9000, 3000)).toBe(6000)
    expect(timeLeftMs(1000, 3000, 1000)).toBeNull()
    expect(timeLeftMs(0, 3000, 5000)).toBeNull()
    expect(timeLeftMs(3000, 3000, 5000)).toBeNull()
  })

  it('rounds the time left the way Explorer does', () => {
    expect(formatTimeLeft(4000)).toBe('Less than 10 s left')
    expect(formatTimeLeft(42_000)).toBe('About 40 s left')
    expect(formatTimeLeft(185_000)).toBe('About 3 min left')
    expect(formatTimeLeft(3_900_000)).toBe('About 1 h 05 min left')
  })
})

describe('not-copied list', () => {
  it('has a header and one Windows path per row with its reason', () => {
    const details = {
      summary: {
        id: 'r1',
        settings: { source: 'C:\\src', destination: 'D:\\dst', mode: 'copy', verify: 'hash' },
        state: 'completed',
        createdAtMs: Date.UTC(2026, 9, 3, 12),
        totals: { notCopied: 2, notCopiedBytes: 2048 },
      },
      notCopied: [item('docs/a.txt', 'fileLocked'), item('b.txt', 'accessDenied')],
    } as unknown as RunDetails

    const lines = formatNotCopiedList(details).split('\r\n')

    expect(lines[0]).toBe('Not copied: 2 items (2.00 KB)')
    expect(lines[1]).toBe('Source: C:\\src')
    expect(lines.slice(-2)).toEqual([
      'docs\\a.txt\tIn use by another program',
      'b.txt\tAccess denied',
    ])
  })
})
