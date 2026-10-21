import type { DiffRow } from './compare'
import { formatCount } from './format'

export type CompareFilter = 'all' | 'changes' | 'missing'

export interface VisibleRow {
  row: DiffRow
  depth: number
  expanded: boolean
  /** The folder row this row sits in; `null` at the top level. */
  parentId: number | null
}

/** `size` is the backend order: largest first. */
export type SortKey = 'size' | 'name' | 'left' | 'right' | 'delta'
export type SortDir = 'asc' | 'desc'

export interface Sort {
  key: SortKey
  dir: SortDir
}

export const defaultSort: Sort = { key: 'size', dir: 'desc' }

const nameOrder = new Intl.Collator('en-US', { numeric: true, sensitivity: 'base' })

/** Next sort when a column header is clicked: a new column starts at its natural direction. */
export function nextSort(current: Sort, key: Exclude<SortKey, 'size'>): Sort {
  if (current.key === key) {
    return { key, dir: current.dir === 'asc' ? 'desc' : 'asc' }
  }
  return { key, dir: key === 'name' || key === 'delta' ? 'asc' : 'desc' }
}

export function sortRows(rows: readonly DiffRow[], sort: Sort): readonly DiffRow[] {
  if (sort.key === 'size') {
    return rows
  }
  const sign = sort.dir === 'asc' ? 1 : -1
  const value = (row: DiffRow): number => {
    switch (sort.key) {
      case 'left':
        return row.left?.size ?? -1
      case 'right':
        return row.right?.size ?? -1
      default:
        return sizeDelta(row)
    }
  }
  return [...rows].sort((a, b) => {
    if (sort.key === 'name') {
      // Folders first, as in Explorer.
      const folders = Number(b.kind === 'dir') - Number(a.kind === 'dir')
      return folders || sign * nameOrder.compare(a.name, b.name)
    }
    return sign * (value(a) - value(b)) || nameOrder.compare(a.name, b.name)
  })
}

/** Whether a row (and so its folder) belongs in the table under `filter`. */
export function matchesFilter(row: DiffRow, filter: CompareFilter): boolean {
  switch (filter) {
    case 'all':
      return true
    case 'changes':
      return row.status !== 'same'
    case 'missing':
      return row.status === 'onlyLeft' || row.missing > 0
  }
}

/**
 * Flattens the loaded part of the tree into table rows, depth first. A folder's rows appear only
 * when it is expanded and its children are loaded.
 */
export function visibleRows(
  roots: readonly DiffRow[],
  children: ReadonlyMap<number, readonly DiffRow[]>,
  expanded: ReadonlySet<number>,
  filter: CompareFilter,
  sort: Sort = defaultSort,
): VisibleRow[] {
  const out: VisibleRow[] = []
  const visit = (rows: readonly DiffRow[], depth: number, parentId: number | null): void => {
    for (const row of sortRows(rows, sort)) {
      if (!matchesFilter(row, filter)) {
        continue
      }
      const isExpanded = row.hasChildren && expanded.has(row.id)
      out.push({ row, depth, expanded: isExpanded, parentId })
      const below = isExpanded ? children.get(row.id) : undefined
      if (below) {
        visit(below, depth + 1, row.id)
      }
    }
  }
  visit(roots, 0, null)
  return out
}

/** Destination minus source; a side that doesn't exist counts as 0. */
export function sizeDelta(row: DiffRow): number {
  return (row.right?.size ?? 0) - (row.left?.size ?? 0)
}

export type Tone = 'success' | 'warning' | 'danger' | 'neutral'

/** Short status text and its colour for the status column. */
export function statusLabel(row: DiffRow): { text: string; tone: Tone } {
  switch (row.status) {
    case 'same':
      return { text: 'Match', tone: 'success' }
    case 'onlyLeft':
      return { text: 'Missing', tone: 'danger' }
    case 'onlyRight':
      return { text: 'Only at destination', tone: 'neutral' }
    case 'kindMismatch':
      return { text: 'Type differs', tone: 'warning' }
    case 'different':
      if (row.kind !== 'dir') {
        return { text: 'Size differs', tone: 'warning' }
      }
      return row.missing > 0
        ? { text: `${formatCount(row.missing)} missing`, tone: 'danger' }
        : { text: 'Differs', tone: 'warning' }
  }
}
