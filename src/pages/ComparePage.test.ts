// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'

const core = vi.hoisted(() => ({ invoke: vi.fn() }))
const reports = vi.hoisted(() => ({ exportReport: vi.fn(), openReport: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  isTauri: () => false,
  Channel: class {
    onmessage: (message: unknown) => void = () => undefined
  },
}))
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }))
// jsdom has no matchMedia, which theme.ts reads at import time.
vi.mock('@/lib/theme', async () => ({
  themeChoice: (await import('vue')).ref('system'),
  setThemeChoice: vi.fn(),
}))
vi.mock('@/lib/reports', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/reports')>()),
  ...reports,
}))

import ComparePage from './ComparePage.vue'
import { compareJob, type CompareResult } from '@/lib/compare'
import { router } from '@/router'

const result: CompareResult = {
  summary: {
    left: { size: 1, files: 1, dirs: 1 },
    right: { size: 1, files: 1, dirs: 1 },
    missing: 0,
    missingBytes: 0,
    extra: 0,
    different: 0,
    leftErrors: 0,
    rightErrors: 0,
    leftCloud: 0,
    rightCloud: 0,
  },
  rows: [],
  elapsedMs: 5,
  reportId: 'r1',
  permissions: {
    status: 'checked',
    elevated: false,
    sourceServer: '',
    destinationServer: '',
    foldersChecked: 1,
    filesChecked: 0,
    skipped: 0,
    counts: [{ status: 'ownerDiffers', count: 1 }],
    itemsTruncated: false,
  },
}

let wrapper: VueWrapper | undefined

async function open(): Promise<VueWrapper> {
  await router.push('/compare')
  await router.isReady()
  Object.assign(compareJob, {
    running: false,
    result,
    compared: { left: 'C:\\a', right: 'D:\\b' },
    finishedAt: new Date(2026, 9, 3, 15, 42),
    error: '',
    notice: '',
    children: new Map(),
    expanded: new Set(),
    loading: new Set(),
  })
  wrapper = mount(ComparePage, { global: { plugins: [router] }, attachTo: document.body })
  await flushPromises()
  return wrapper
}

function click(page: VueWrapper, text: string): Promise<void> {
  const found = page.findAll('button').find((button) => button.text().includes(text))
  if (!found) throw new Error(`no ${text} button`)
  return found.trigger('click')
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
})
afterEach(() => {
  wrapper?.unmount()
  wrapper = undefined
  Object.assign(compareJob, { result: null, running: false })
})

describe('ComparePage permissions', () => {
  it('turns the pass on only when the box is ticked', async () => {
    const page = await open()
    core.invoke.mockReturnValue(new Promise(() => undefined))
    await click(page, 'Re-check')
    expect(core.invoke).toHaveBeenCalledWith(
      'compare_start',
      expect.objectContaining({ comparePermissions: false }),
    )
    // The control is off by default.
    expect(compareJob.running).toBe(true)
  })

  it('says Saved after the export finishes', async () => {
    const page = await open()
    reports.exportReport.mockResolvedValue('C:\\out\\perms.csv')
    await click(page, 'Permissions CSV')
    await flushPromises()
    expect(reports.exportReport).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'compare', id: 'r1' }),
      'csv',
      'permissions',
    )
    expect(document.body.textContent).toContain('Saved C:\\out\\perms.csv')
  })

  it('stays quiet when a newer compare started while the export was being written', async () => {
    const page = await open()
    let finish: (path: string | null) => void = () => undefined
    reports.exportReport.mockReturnValue(
      new Promise<string | null>((resolve) => {
        finish = resolve
      }),
    )
    await click(page, 'Permissions CSV')
    // A newer compare replaces the result before the export finishes.
    core.invoke.mockReturnValue(new Promise(() => undefined))
    await click(page, 'Re-check')
    finish('C:\\out\\old.csv')
    await flushPromises()
    expect(document.body.textContent).not.toContain('Saved')
  })

  it('does not show an export error for a result that is gone', async () => {
    const page = await open()
    let fail: (reason: unknown) => void = () => undefined
    reports.exportReport.mockReturnValue(
      new Promise<string | null>((_, reject) => {
        fail = reject
      }),
    )
    await click(page, 'Permissions CSV')
    core.invoke.mockReturnValue(new Promise(() => undefined))
    await click(page, 'Re-check')
    fail(new Error('disk full'))
    await flushPromises()
    expect(document.body.textContent).not.toContain('disk full')
  })
})
