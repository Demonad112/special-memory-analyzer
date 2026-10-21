import { describe, expect, it } from 'vitest'
import { squarify, type Rect } from './treemap'

const area = (rect: Rect): number => rect.width * rect.height
const close = (a: number, b: number): boolean => Math.abs(a - b) < 1e-6

describe('squarify', () => {
  it('lays out the paper example with exact areas, inside the rect, without overlaps', () => {
    const items = [6, 6, 4, 3, 2, 2, 1].map((size) => ({ size }))
    const rect = { x: 10, y: 20, width: 6, height: 4 }
    const tiles = squarify(items, rect)

    expect(tiles).toHaveLength(7)
    tiles.forEach((tile) => {
      expect(close(area(tile), tile.item.size)).toBe(true)
      expect(tile.x).toBeGreaterThanOrEqual(rect.x - 1e-6)
      expect(tile.y).toBeGreaterThanOrEqual(rect.y - 1e-6)
      expect(tile.x + tile.width).toBeLessThanOrEqual(rect.x + rect.width + 1e-6)
      expect(tile.y + tile.height).toBeLessThanOrEqual(rect.y + rect.height + 1e-6)
    })
    for (const [i, a] of tiles.entries()) {
      for (const b of tiles.slice(i + 1)) {
        const overlapX = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x)
        const overlapY = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y)
        expect(overlapX <= 1e-6 || overlapY <= 1e-6).toBe(true)
      }
    }
    // The first two sixes share the first column, as in the paper.
    expect(close(tiles[0]?.width ?? 0, 3)).toBe(true)
    expect(close(tiles[1]?.x ?? 0, 10)).toBe(true)
  })

  it('keeps tiles close to square', () => {
    const items = Array.from({ length: 40 }, (_, index) => ({ size: 1000 / (index + 1) }))
    const tiles = squarify(items, { x: 0, y: 0, width: 800, height: 400 })
    const worst = Math.max(
      ...tiles.map((tile) => Math.max(tile.width / tile.height, tile.height / tile.width)),
    )
    expect(worst).toBeLessThan(5)
  })

  it('scales sizes to the rect and skips empty items', () => {
    const tiles = squarify([{ size: 3e9 }, { size: 0 }, { size: 1e9 }], {
      x: 0,
      y: 0,
      width: 100,
      height: 100,
    })
    expect(tiles.map((tile) => Math.round(area(tile)))).toEqual([7500, 2500])
  })

  it('returns nothing for no size or no room', () => {
    expect(squarify([{ size: 0 }], { x: 0, y: 0, width: 10, height: 10 })).toEqual([])
    expect(squarify([{ size: 5 }], { x: 0, y: 0, width: 0, height: 10 })).toEqual([])
  })
})
