// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref } from 'vue'

const record = vi.hoisted(() => ({ loadRecord: vi.fn() }))
const totals = {
  plannedFiles: 4,
  plannedBytes: 40,
  copied: 4,
  copiedBytes: 40,
  skippedIdentical: 0,
  notCopied: 0,
  notCopiedBytes: 0,
  changed: 1,
  new: 2,
  retried: 1,
  unchanged: 30,
  changedAtDestination: 1,
  destinationOnly: 12,
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: () => true,
  Channel: class {},
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@/lib/record', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/record')>()),
  loadRecord: record.loadRecord,
}))

import RecordReportPage from './RecordReportPage.vue'

function details(source: string): unknown {
  return { summary: { settings: { source, destination: 'D:\out', mode: 'copy' } }, notCopied: [] }
}

describe('RecordReportPage', () => {
  afterEach(() => {
    record.loadRecord.mockReset()
    document.body.innerHTML = ''
  })

  it('clears the old run while a new one loads and ignores a slower earlier load', async () => {
    let releaseFirst: (value: unknown) => void = () => undefined
    record.loadRecord.mockImplementation((id: string) =>
      id === 'a'
        ? new Promise((resolve) => {
            releaseFirst = resolve
          })
        : Promise.resolve(details('C:\second')),
    )
    const id = ref('a')
    const host = document.createElement('div')
    document.body.append(host)
    createApp(defineComponent({ render: () => h(RecordReportPage, { id: id.value }) })).mount(host)

    id.value = 'b'
    await nextTick()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(host.textContent).toContain('C:\second')

    releaseFirst(details('C:\first'))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(host.textContent).toContain('C:\second')
    expect(host.textContent).not.toContain('C:\first')
  })

  async function mountReport(extra: Record<string, unknown>, mode = 'copy', state = 'completed') {
    record.loadRecord.mockResolvedValue({
      summary: {
        id: 'r2',
        settings: { source: 'C:\\src', destination: 'D:\\out', mode },
        state,
        createdAtMs: 1,
        totals,
        ...extra,
      },
      preflight: null,
      notCopied: [],
      leftAtDestination: [],
      previousRun: null,
    })
    const host = document.createElement('div')
    document.body.append(host)
    createApp(defineComponent({ render: () => h(RecordReportPage, { id: 'r2' }) })).mount(host)
    await new Promise((resolve) => setTimeout(resolve, 0))
    return host
  }

  it('offers "Copy changes since this run" on a finished copy and hides it for a watched run', async () => {
    const copied = await mountReport({})
    expect(copied.textContent).toContain('Copy changes since this run')
    document.body.innerHTML = ''
    const watched = await mountReport({}, 'watch')
    expect(watched.textContent).not.toContain('Copy changes since this run')
    document.body.innerHTML = ''
    const running = await mountReport({}, 'copy', 'running')
    expect(running.textContent).not.toContain('Copy changes since this run')
  })

  it('shows the chain, the totals and what was left alone for a delta run, and nothing for an old run', async () => {
    record.loadRecord.mockResolvedValue({
      summary: {
        id: 'r2',
        settings: { source: 'C:\\src', destination: 'D:\\out', mode: 'copy' },
        state: 'completed',
        createdAtMs: 1,
        totals,
        previousRunId: 'r1',
      },
      preflight: null,
      notCopied: [],
      previousRun: { id: 'r1', createdAtMs: Date.UTC(2026, 9, 21, 18, 2), finishedAtMs: 2 },
      leftAtDestination: [
        {
          relativePath: 'docs/edited.txt',
          size: 5,
          sourceModifiedMs: 1,
          destSize: 9,
          destModifiedMs: 5,
        },
      ],
    })
    const host = document.createElement('div')
    document.body.append(host)
    createApp(defineComponent({ render: () => h(RecordReportPage, { id: 'r2' }) })).mount(host)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(host.textContent).toContain('Changes since run of')
    expect(host.textContent).toContain('1 changed · 2 new · 1 retried · 30 unchanged')
    expect(host.textContent).toContain('12 files only at the destination, left in place')
    expect(host.textContent).toContain('Changed at the destination since the last run (1)')
    expect(host.textContent).toContain('docs\\edited.txt')

    document.body.innerHTML = ''
    const old = await mountReport({ previousRunId: null })
    expect(old.textContent).not.toContain('Changes since run')
    expect(old.textContent).not.toContain('Changed at the destination')
  })
})
