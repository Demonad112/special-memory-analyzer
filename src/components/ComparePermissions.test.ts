// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import CompareTable from './CompareTable.vue'
import CompareToolbar from './CompareToolbar.vue'
import PermissionDetailPanel from './PermissionDetailPanel.vue'
import type {
  CompareResult,
  DiffRow,
  PermCell,
  PermissionDetail,
  PermissionEntry,
} from '@/lib/compare'
import { defaultSort, type VisibleRow } from '@/lib/diffRows'

function diffRow(id: number, permissions?: PermCell): DiffRow {
  return {
    id,
    name: `folder${String(id)}`,
    kind: 'dir',
    left: { size: 10, files: 1, dirs: 0 },
    right: { size: 10, files: 1, dirs: 0 },
    status: 'same',
    missing: 0,
    missingBytes: 0,
    extra: 0,
    different: 0,
    cloud: false,
    error: null,
    lossyName: false,
    hasChildren: false,
    ...(permissions ? { permissions } : {}),
  }
}

const visible = (row: DiffRow): VisibleRow => ({ row, depth: 0, expanded: false, parentId: null })

function table(rows: DiffRow[], showPermissions: boolean) {
  return mount(CompareTable, {
    props: {
      rows: rows.map(visible),
      loading: new Set<number>(),
      sort: defaultSort,
      selected: null,
      showPermissions,
    },
  })
}

describe('CompareTable permissions column', () => {
  const rows = [
    diffRow(1, { status: 'ownerDiffers', below: 0, summary: 'Owner: A → B' }),
    diffRow(2, { status: 'sameNameDifferentAccount', below: 0, summary: '' }),
    diffRow(3, { status: 'same', below: 0, summary: '' }),
    diffRow(4),
  ]

  it('adds the column and a chip per compared row when the compare looked', () => {
    const wrapper = table(rows, true)
    expect(wrapper.text()).toContain('Permissions')
    const cells = wrapper.findAll('[data-testid="permissions-cell"]')
    expect(cells.map((cell) => cell.text())).toEqual([
      'Owner differs',
      'Same name, different account',
      'Same',
      '',
    ])
    expect(cells[0]?.attributes('title')).toBe('Owner: A → B')
    // Amber for the same-name one, red for a real difference.
    expect(cells[1]?.find('span').classes()).toContain('text-warning')
    expect(cells[0]?.find('span').classes()).toContain('text-danger')
  })

  it('leaves the table exactly as before when it did not', () => {
    const wrapper = table(rows, false)
    expect(wrapper.find('[data-testid="permissions-cell"]').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Permissions')
    expect(wrapper.find('.compare-table').classes()).not.toContain('perms')
  })
})

function result(permissions: CompareResult['permissions']): CompareResult {
  return {
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
    ...(permissions ? { permissions } : {}),
  }
}

const checked: NonNullable<CompareResult['permissions']> = {
  status: 'checked',
  elevated: false,
  sourceServer: '',
  destinationServer: '',
  foldersChecked: 3,
  filesChecked: 0,
  skipped: 0,
  counts: [{ status: 'ownerDiffers', count: 1 }],
  itemsTruncated: false,
}

function toolbar(permissions: CompareResult['permissions']) {
  return mount(CompareToolbar, {
    props: {
      result: result(permissions),
      compared: { left: 'C:\\a', right: 'D:\\b' },
      finishedAt: new Date(2026, 9, 3, 15, 42),
      filter: 'all',
      confirming: false,
    },
  })
}

describe('CompareToolbar permissions', () => {
  it('offers the permissions filter and CSV only when the compare checked them', async () => {
    const without = toolbar(undefined)
    expect(without.text()).not.toContain('Permissions')
    const wrapper = toolbar(checked)
    const radios = wrapper.findAll('[role="radio"]').map((radio) => radio.text())
    expect(radios).toEqual(['All', 'Differences', 'Missing', 'Permissions'])
    await wrapper.findAll('[role="radio"]')[3]?.trigger('click')
    expect(wrapper.emitted('filter')?.[0]).toEqual(['permissions'])
    const csv = wrapper.findAll('button').find((button) => button.text() === 'Permissions CSV')
    await csv?.trigger('click')
    expect(wrapper.emitted('exportPermissions')).toHaveLength(1)
  })

  it('does not offer them off Windows', () => {
    const wrapper = toolbar({ ...checked, status: 'notSupported' })
    expect(wrapper.text()).not.toContain('Permissions')
  })

  it('disables the CSV while it is being written', () => {
    const wrapper = mount(CompareToolbar, {
      props: {
        result: result(checked),
        compared: { left: 'a', right: 'b' },
        finishedAt: null,
        filter: 'all',
        confirming: false,
        exporting: true,
      },
    })
    const csv = wrapper.findAll('button').find((button) => button.text() === 'Permissions CSV')
    expect(csv?.attributes('disabled')).toBeDefined()
  })
})

const entry = (account: string): PermissionEntry => ({
  account,
  sid: `S-${account}`,
  access: 'Allow',
  rights: 'Modify',
  mask: 0x1301bf,
  inherited: false,
  appliesTo: 'This folder only',
})

const detail: PermissionDetail = {
  path: 'Finance',
  dir: true,
  status: 'entriesDiffer',
  sourceOwner: 'SRV\\alice',
  destinationOwner: 'S-1-5-18',
  sourceProtected: true,
  destinationProtected: false,
  note: '',
  error: '',
  missing: [entry('SRV\\bob')],
  extra: [entry('Everyone')],
  sameName: [{ source: entry('OLD\\carol'), destination: entry('NEW\\carol') }],
}

describe('PermissionDetailPanel', () => {
  const cell: PermCell = { status: 'entriesDiffer', below: 0, summary: 's' }

  it('lists the owner, inheritance and every entry that differs', () => {
    const text = mount(PermissionDetailPanel, {
      props: { name: 'Finance', cell, detail, loading: false },
    }).text()
    expect(text).toContain('Permissions of Finance')
    expect(text).toContain('Owner: SRV\\alice at the source, S-1-5-18 at the destination')
    expect(text).toContain('off at the source, on at the destination')
    expect(text).toContain('Missing at the destination: SRV\\bob: Allow Modify (This folder only)')
    expect(text).toContain('Only at the destination: Everyone')
    expect(text).toContain('Same name, different account: OLD\\carol')
  })

  it('says so when the details were not kept, and while loading', () => {
    const kept = mount(PermissionDetailPanel, {
      props: {
        name: 'x',
        cell: { status: 'ownerDiffers', below: 0, summary: '' },
        detail: null,
        loading: false,
      },
    })
    expect(kept.text()).toContain('not kept')
    const folder = mount(PermissionDetailPanel, {
      props: {
        name: 'x',
        cell: { status: 'same', below: 4, summary: '' },
        detail: null,
        loading: false,
      },
    })
    expect(folder.text()).toContain('4 item(s) below this folder differ')
    const loading = mount(PermissionDetailPanel, {
      props: { name: 'x', cell, detail: null, loading: true },
    })
    expect(loading.text()).toContain('Loading')
  })
})
