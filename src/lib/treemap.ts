export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

export type Tile<T> = Rect & { item: T }

/**
 * Squarified treemap (Bruls, Huizing, van Wijk): lays items out in `rect` with areas in
 * proportion to their sizes and tiles as close to square as it can. Items should come largest
 * first; zero sizes are left out.
 */
export function squarify<T extends { size: number }>(items: T[], rect: Rect): Tile<T>[] {
  const sized = items.filter((item) => item.size > 0)
  const total = sized.reduce((sum, item) => sum + item.size, 0)
  if (total <= 0 || rect.width <= 0 || rect.height <= 0) {
    return []
  }
  const scale = (rect.width * rect.height) / total
  const tiles: Tile<T>[] = []
  let free = { ...rect }
  let row: T[] = []

  const areaOf = (list: T[]): number => list.reduce((sum, item) => sum + item.size * scale, 0)
  // Worst aspect ratio of a row laid along a side of this length.
  const worst = (list: T[], side: number): number => {
    const sum = areaOf(list)
    const areas = list.map((item) => item.size * scale)
    const max = Math.max(...areas)
    const min = Math.min(...areas)
    return Math.max((side * side * max) / (sum * sum), (sum * sum) / (side * side * min))
  }
  const place = (list: T[]): void => {
    const sum = areaOf(list)
    if (free.width >= free.height) {
      // A column on the left.
      const width = Math.min(free.width, sum / free.height)
      let y = free.y
      for (const item of list) {
        const height = (item.size * scale) / width
        tiles.push({ x: free.x, y, width, height, item })
        y += height
      }
      free = { ...free, x: free.x + width, width: free.width - width }
    } else {
      // A row along the top.
      const height = Math.min(free.height, sum / free.width)
      let x = free.x
      for (const item of list) {
        const width = (item.size * scale) / height
        tiles.push({ x, y: free.y, width, height, item })
        x += width
      }
      free = { ...free, y: free.y + height, height: free.height - height }
    }
  }

  for (const item of sized) {
    const side = Math.min(free.width, free.height)
    if (row.length === 0 || worst([...row, item], side) <= worst(row, side)) {
      row.push(item)
    } else {
      place(row)
      row = [item]
    }
  }
  if (row.length > 0) {
    place(row)
  }
  return tiles
}
