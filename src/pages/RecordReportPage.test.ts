// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref } from 'vue'

const record = vi.hoisted(() => ({ loadRecord: vi.fn() }))

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
})
