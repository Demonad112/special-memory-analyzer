const units = ['bytes', 'KB', 'MB', 'GB', 'TB', 'PB']

/** Size as Explorer shows it: 1024-based, up to three significant digits ("1.25 GB", "940 KB"). */
export function formatBytes(bytes: number): string {
  const sign = bytes < 0 ? '-' : ''
  let value = Math.abs(bytes)
  if (value < 1024) {
    return `${sign}${String(value)} ${value === 1 ? 'byte' : 'bytes'}`
  }
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2
  // Rounding can reach 1024 (e.g. 1023.9 KB); show the next unit instead.
  if (Number(value.toFixed(digits)) >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
    return `${sign}${value.toFixed(2)} ${units[unit] ?? ''}`
  }
  return `${sign}${value.toFixed(digits)} ${units[unit] ?? ''}`
}

/** Signed size difference: "+1.20 MB", "-512 bytes", "0 bytes" when equal. */
export function formatDelta(bytes: number): string {
  if (bytes === 0) {
    return '0 bytes'
  }
  return bytes > 0 ? `+${formatBytes(bytes)}` : formatBytes(bytes)
}

const countFormat = new Intl.NumberFormat('en-US')

export function formatCount(count: number): string {
  return countFormat.format(count)
}

/** "1 file", "2,048 files". */
export function plural(count: number, one: string, many = `${one}s`): string {
  return `${formatCount(count)} ${count === 1 ? one : many}`
}

/** Elapsed time for a finished job: "850 ms", "4.2 s", "3 min 05 s". */
export function formatDuration(ms: number): string {
  if (ms < 1000) {
    return `${String(Math.round(ms))} ms`
  }
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(1)} s`
  }
  const minutes = Math.floor(ms / 60_000)
  const seconds = Math.round((ms % 60_000) / 1000)
  return `${String(minutes)} min ${String(seconds).padStart(2, '0')} s`
}

/** "m:ss" for a running timer; negative values count as zero. */
export function formatClock(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000))
  return `${String(Math.floor(seconds / 60))}:${String(seconds % 60).padStart(2, '0')}`
}

/** "Oct 23, 2026, 5:07 PM": the one date-and-time format used in the app. */
export function formatDateTime(when: Date | number): string {
  return new Date(when).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })
}

/** "Oct 23, 2026". */
export function formatDate(when: Date | number): string {
  return new Date(when).toLocaleDateString('en-US', { dateStyle: 'medium' })
}

/** "5:07 PM". */
export function formatTime(when: Date | number): string {
  return new Date(when).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })
}

/** `size` as a share of `total`: "42%", one decimal under 10% ("3.5%"), "0%" for an empty total. */
export function percentShare(size: number, total: number): string {
  return total > 0 ? `${((size / total) * 100).toFixed(size / total >= 0.1 ? 0 : 1)}%` : '0%'
}
