import { describe, expect, it } from 'vitest'
import type { DiffRow, PermCell } from './compare'
import {
  defaultSort,
  hasPermissionDifference,
  matchesFilter,
  nextSort,
  permissionLabel,
  sizeDelta,
  sortRows,
  statusLabel,
  visibleRows,
} from './diffRows'

function row(id: number, overrides: Partial<DiffRow> = {}): DiffRow {
  return {
    id,
    name: `row${String(id)}`,
    kind: 'file',
    left: { size: 10, files: 1, dirs: 0 },
    right: { size: 10, files: 1, dirs: 0 },
    status: 'same',
    missing: 0,
    missingBytes: 0,
    extra: 0,
    different: 0,
    cloud: false,
    lossyName: false,
    error: null,
    hasChildren: false,
    ...overrides,
  }
}

const folder = row(1, { kind: 'dir', status: 'different', missing: 1, hasChildren: true })
const same = row(2)
const lost = row(3, { status: 'onlyLeft', right: null, missing: 1, missingBytes: 10 })
const extra = row(4, { status: 'onlyRight', left: null, extra: 1 })
const changed = row(5, { status: 'different', different: 1, right: { size: 4, files: 1, dirs: 0 } })

describe('visibleRows', () => {
  const children = new Map([[1, [lost, same]]])

  it('shows only top-level rows until a folder is expanded', () => {
    const rows = visibleRows([folder, extra], children, new Set(), 'all')
    expect(rows.map((visible) => visible.row.id)).toEqual([1, 4])
    expect(rows[0]?.expanded).toBe(false)
  })

  it('inserts loaded children after an expanded folder, one level deeper', () => {
    const rows = visibleRows([folder, extra], children, new Set([1]), 'all')
    expect(rows.map((visible) => [visible.row.id, visible.depth])).toEqual([
      [1, 0],
      [3, 1],
      [2, 1],
      [4, 0],
    ])
    expect(rows[0]?.expanded).toBe(true)
    expect(rows.map((visible) => visible.parentId)).toEqual([null, 1, 1, null])
  })

  it('sorts every level', () => {
    const big = row(4, { right: { size: 50, files: 1, dirs: 0 } })
    const small = row(2, { right: { size: 5, files: 1, dirs: 0 } })
    const rows = visibleRows([folder, big], new Map([[1, [small, lost]]]), new Set([1]), 'all', {
      key: 'right',
      dir: 'desc',
    })
    // `lost` has no destination side, so it sorts last.
    expect(rows.map((visible) => visible.row.id)).toEqual([4, 1, 2, 3])
  })

  it('keeps an expanded folder whose children are still loading', () => {
    const rows = visibleRows([folder], new Map(), new Set([1]), 'all')
    expect(rows).toHaveLength(1)
  })

  it('filters at every level', () => {
    const missing = visibleRows([folder, extra, changed], children, new Set([1]), 'missing')
    expect(missing.map((visible) => visible.row.id)).toEqual([1, 3])
    const changes = visibleRows([folder, extra, changed], children, new Set([1]), 'changes')
    expect(changes.map((visible) => visible.row.id)).toEqual([1, 3, 4, 5])
  })
})

describe('row helpers', () => {
  it('matches filters', () => {
    expect(matchesFilter(same, 'all')).toBe(true)
    expect(matchesFilter(same, 'changes')).toBe(false)
    expect(matchesFilter(extra, 'missing')).toBe(false)
    expect(matchesFilter(row(9, { kind: 'dir', status: 'onlyLeft', missing: 0 }), 'missing')).toBe(
      true,
    )
  })

  it('computes destination minus source', () => {
    expect(sizeDelta(changed)).toBe(-6)
    expect(sizeDelta(lost)).toBe(-10)
    expect(sizeDelta(extra)).toBe(10)
  })

  it('labels statuses', () => {
    expect(statusLabel(same)).toEqual({ text: 'Match', tone: 'success' })
    expect(statusLabel(lost).tone).toBe('danger')
    expect(statusLabel(changed).text).toBe('Size differs')
    expect(statusLabel(folder)).toEqual({ text: '1 missing', tone: 'danger' })
    expect(statusLabel(row(7, { kind: 'dir', status: 'different' })).text).toBe('Differs')
  })
})

describe('sorting', () => {
  it('keeps the backend order by default', () => {
    expect(sortRows([same, lost, extra], defaultSort).map((r) => r.id)).toEqual([2, 3, 4])
  })

  it('sorts names naturally with folders first', () => {
    const rows = [
      row(1, { name: 'file10.txt' }),
      row(2, { name: 'file9.txt' }),
      row(3, { name: 'Zeta', kind: 'dir' }),
    ]
    expect(sortRows(rows, { key: 'name', dir: 'asc' }).map((r) => r.id)).toEqual([3, 2, 1])
    expect(sortRows(rows, { key: 'name', dir: 'desc' }).map((r) => r.id)).toEqual([3, 1, 2])
  })

  it('sorts by difference, biggest loss first', () => {
    expect(sortRows([extra, changed, lost], { key: 'delta', dir: 'asc' }).map((r) => r.id)).toEqual(
      [3, 5, 4],
    )
  })

  it('cycles header clicks', () => {
    expect(nextSort(defaultSort, 'name')).toEqual({ key: 'name', dir: 'asc' })
    expect(nextSort({ key: 'name', dir: 'asc' }, 'name')).toEqual({ key: 'name', dir: 'desc' })
    expect(nextSort(defaultSort, 'left')).toEqual({ key: 'left', dir: 'desc' })
  })
})

describe('permissions', () => {
  const cell = (status: PermCell['status'], below = 0): PermCell => ({ status, below, summary: '' })

  it('filters to rows that differ or hold differences below them', () => {
    const differs = row(10, { permissions: cell('ownerDiffers') })
    const holder = row(11, { kind: 'dir', permissions: cell('same', 2) })
    const unchecked = row(12, { kind: 'dir', permissions: cell(null, 1) })
    const clean = row(13, { permissions: cell('same') })
    const plain = row(14)
    const shown = [differs, holder, unchecked, clean, plain].filter((candidate) =>
      matchesFilter(candidate, 'permissions'),
    )
    expect(shown.map((candidate) => candidate.id)).toEqual([10, 11, 12])
    expect(hasPermissionDifference(undefined)).toBe(false)
    expect(hasPermissionDifference(cell('sameNameDifferentAccount'))).toBe(true)
    expect(hasPermissionDifference(cell('notReadable'))).toBe(true)
  })

  it('the other filters ignore permissions entirely', () => {
    const differs = row(10, { permissions: cell('entriesDiffer') })
    expect(matchesFilter(differs, 'changes')).toBe(false)
    expect(matchesFilter(differs, 'missing')).toBe(false)
    expect(matchesFilter(differs, 'all')).toBe(true)
  })

  it('labels each status, with the same-name one in amber and the real ones in red', () => {
    expect(permissionLabel(undefined)).toBeNull()
    expect(permissionLabel(cell(null))).toBeNull()
    expect(permissionLabel(cell('same'))).toEqual({ text: 'Same', tone: 'success' })
    expect(permissionLabel(cell('ownerDiffers'))).toEqual({ text: 'Owner differs', tone: 'danger' })
    expect(permissionLabel(cell('entriesDiffer'))?.tone).toBe('danger')
    expect(permissionLabel(cell('inheritanceDiffers'))?.text).toBe('Inheritance differs')
    expect(permissionLabel(cell('sameNameDifferentAccount'))).toEqual({
      text: 'Same name, different account',
      tone: 'warning',
    })
    expect(permissionLabel(cell('notReadable'))?.tone).toBe('warning')
    expect(permissionLabel(cell(null, 1200))).toEqual({
      text: '1,200 differ below',
      tone: 'warning',
    })
    expect(permissionLabel(cell('same', 3))?.text).toBe('3 differ below')
    // Its own difference wins over the count below it.
    expect(permissionLabel(cell('entriesDiffer', 3))?.text).toBe('Entries differ')
  })
})
