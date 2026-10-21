import { describe, expect, it } from 'vitest'
import { formatBytes, formatCount, formatDelta, formatDuration, plural } from './format'

describe('formatBytes', () => {
  it('keeps small sizes in bytes', () => {
    expect(formatBytes(0)).toBe('0 bytes')
    expect(formatBytes(1)).toBe('1 byte')
    expect(formatBytes(1023)).toBe('1023 bytes')
  })

  it('uses 1024-based units with three significant digits', () => {
    expect(formatBytes(1024)).toBe('1.00 KB')
    expect(formatBytes(1536)).toBe('1.50 KB')
    expect(formatBytes(20 * 1024)).toBe('20.0 KB')
    expect(formatBytes(940 * 1024)).toBe('940 KB')
    expect(formatBytes(1.25 * 1024 ** 3)).toBe('1.25 GB')
  })

  it('moves to the next unit when rounding reaches 1024', () => {
    expect(formatBytes(1024 * 1024 - 1)).toBe('1.00 MB')
  })

  it('keeps the sign of negative sizes', () => {
    expect(formatBytes(-2048)).toBe('-2.00 KB')
  })
})

describe('formatDelta', () => {
  it('signs differences', () => {
    expect(formatDelta(0)).toBe('0 bytes')
    expect(formatDelta(2048)).toBe('+2.00 KB')
    expect(formatDelta(-10)).toBe('-10 bytes')
  })
})

describe('counts and durations', () => {
  it('groups thousands and pluralises', () => {
    expect(formatCount(1234567)).toBe('1,234,567')
    expect(plural(1, 'file')).toBe('1 file')
    expect(plural(2048, 'file')).toBe('2,048 files')
    expect(plural(2, 'folder', 'folders')).toBe('2 folders')
  })

  it('formats elapsed time', () => {
    expect(formatDuration(850)).toBe('850 ms')
    expect(formatDuration(4200)).toBe('4.2 s')
    expect(formatDuration(185_000)).toBe('3 min 05 s')
  })
})
