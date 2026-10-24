import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const palette =
  /\b(?:bg|text|border|ring|fill|stroke|from|to|via|divide|outline|decoration|accent|caret)-(?:white|black|red|gray|slate|zinc|neutral|stone|blue|green|amber|yellow|orange|emerald|sky|indigo|violet|purple|pink|rose|cyan|teal|lime|fuchsia)(?:-\d+)?\b/
const hex = /#[0-9a-fA-F]{3,8}\b/

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      return sources(path)
    }
    return /\.vue$/.test(entry.name) ? [path] : []
  })
}

describe('colour tokens', () => {
  it('uses no Tailwind palette class or raw hex colour in a component', () => {
    const offenders = sources('src').filter((path) => {
      const text = readFileSync(path, 'utf8')
      return palette.test(text) || hex.test(text)
    })
    expect(offenders).toEqual([])
  })

  it('turns the default Tailwind palette off', () => {
    expect(readFileSync('src/styles.css', 'utf8')).toContain('--color-*: initial;')
  })
})
