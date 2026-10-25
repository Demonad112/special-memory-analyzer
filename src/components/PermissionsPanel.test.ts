// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const core = vi.hoisted(() => ({ invoke: vi.fn() }))
const dialog = vi.hoisted(() => ({ save: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  isTauri: () => true,
  Channel: class {},
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: dialog.save, open: vi.fn() }))

import PermissionsPanel from './PermissionsPanel.vue'
import type { PermissionsSummary } from '@/lib/readiness'

const summary: PermissionsSummary = {
  status: 'checked',
  everyFile: false,
  elevated: true,
  server: 'NAS',
  foldersRead: 12,
  filesRead: 0,
  skipped: 0,
  issues: 3,
  counts: [
    { kind: 'wideWrite', count: 2 },
    { kind: 'brokenInheritance', count: 1 },
  ],
  itemsTruncated: false,
}

function panel(props: Partial<{ permissions: PermissionsSummary; reportId: string | null }> = {}) {
  return mount(PermissionsPanel, {
    props: { permissions: summary, reportId: 'r1', view: 1, ...props },
  })
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('PermissionsPanel', () => {
  it('shows counts per finding type in report order, and what ran', () => {
    const text = panel().text()
    expect(text).toContain('12 folders')
    expect(text).toContain('as administrator')
    expect(text).toContain('names asked of NAS')
    expect(text.indexOf('Inheritance broken · 1')).toBeLessThan(
      text.indexOf('Wide write access · 2'),
    )
    expect(text).toContain('Folders only')
  })

  it('loads the list of one type when it is opened', async () => {
    core.invoke.mockResolvedValue({
      total: 2,
      findings: [
        {
          path: 'Payroll',
          dir: true,
          kind: 'wideWrite',
          detail: 'Everyone: Modify',
          sid: 'S-1-1-0',
        },
      ],
    })
    const wrapper = panel()
    const chip = wrapper.findAll('button').find((b) => b.text().startsWith('Wide write'))
    await chip?.trigger('click')
    await flushPromises()
    expect(core.invoke).toHaveBeenCalledWith('readiness_permissions', {
      kind: 'wideWrite',
      offset: 0,
      limit: 200,
    })
    expect(wrapper.text()).toContain('Payroll\\')
    expect(wrapper.text()).toContain('Everyone: Modify')
    expect(wrapper.text()).toContain('1 of 2 shown')
  })

  it('drops a page that arrives after a new check', async () => {
    let answer: (page: unknown) => void = () => undefined
    core.invoke.mockReturnValue(new Promise((resolve) => (answer = resolve)))
    const wrapper = panel()
    await wrapper
      .findAll('button')
      .find((b) => b.text().startsWith('Wide write'))
      ?.trigger('click')
    await wrapper.setProps({ view: 2 })
    answer({
      total: 1,
      findings: [{ path: 'Late', dir: true, kind: 'wideWrite', detail: 'x', sid: '' }],
    })
    await flushPromises()
    expect(wrapper.text()).not.toContain('Late')
  })

  it('exports the permissions CSV layout of the saved report', async () => {
    dialog.save.mockResolvedValueOnce('C:\\out\\p.csv')
    core.invoke.mockResolvedValue(undefined)
    const wrapper = panel()
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('Export permissions'))
      ?.trigger('click')
    await flushPromises()
    expect(core.invoke).toHaveBeenCalledWith(
      'report_export',
      expect.objectContaining({
        kind: 'readiness',
        id: 'r1',
        format: 'csv',
        layout: 'permissions',
      }),
    )
    expect(wrapper.text()).toContain('Saved C:\\out\\p.csv')
  })

  it('cannot export when the report was not saved, and says when it did not run', () => {
    const noReport = panel({ reportId: null })
    const button = noReport.findAll('button').find((b) => b.text().includes('Export permissions'))
    expect(button?.attributes('disabled')).toBeDefined()
    const off = panel({ permissions: { ...summary, status: 'off', counts: [] } })
    expect(off.text()).toContain('Not checked')
    expect(off.text()).not.toContain('Export permissions')
  })
})
