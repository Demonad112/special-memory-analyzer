import { describe, expect, it } from 'vitest'
import { errorMessage } from './errors'

describe('errorMessage', () => {
  it('returns the text of a string, an Error or a { message } object', () => {
    expect(errorMessage('Source: nope')).toBe('Source: nope')
    expect(errorMessage(new Error('boom'))).toBe('boom')
    expect(errorMessage({ message: 'structured' })).toBe('structured')
  })

  it('falls back for anything else', () => {
    expect(errorMessage(undefined)).toBe('Something went wrong.')
    expect(errorMessage(42)).toBe('Something went wrong.')
    expect(errorMessage({ message: 7 })).toBe('Something went wrong.')
    expect(errorMessage(null)).toBe('Something went wrong.')
  })
})
