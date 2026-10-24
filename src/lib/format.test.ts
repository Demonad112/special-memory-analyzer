import { describe, expect, it } from 'vitest'
import {
  formatBytes,
  formatClock,
  formatCount,
  formatDate,
  formatDateTime,
  formatDelta,
  formatReportDateTime,
  formatDuration,
  formatTime,
  percentShare,
  plural,
} from './format'

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

describe('clock and date formats', () => {
  it('formats a running timer', () => {
    expect(formatClock(0)).toBe('0:00')
    expect(formatClock(65_000)).toBe('1:05')
    expect(formatClock(-5000)).toBe('0:00')
  })

  it('formats dates in the user locale, the same way everywhere', () => {
    const when = new Date(2026, 9, 23, 17, 7)
    expect(formatDateTime(when)).toBe(
      when.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }),
    )
    expect(formatDate(when)).toBe(when.toLocaleDateString(undefined, { dateStyle: 'medium' }))
    expect(formatTime(when)).toBe(
      when.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }),
    )
    expect(formatDateTime(when.getTime())).toBe(formatDateTime(when))
    expect(formatReportDateTime(when)).toBe('Oct 23, 2026, 5:07 PM')
  })
})

describe('percentShare', () => {
  it('rounds to a whole percent from 10% up and shows one decimal below', () => {
    expect(percentShare(50, 100)).toBe('50%')
    expect(percentShare(10, 100)).toBe('10%')
    expect(percentShare(35, 1000)).toBe('3.5%')
    expect(percentShare(0, 100)).toBe('0.0%')
  })

  it('is 0% when the total is empty', () => {
    expect(percentShare(5, 0)).toBe('0%')
  })
})
