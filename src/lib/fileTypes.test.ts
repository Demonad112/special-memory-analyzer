import { describe, expect, it } from 'vitest'
import { categoryOf, categoryOfExtension } from './fileTypes'

describe('file types', () => {
  it('groups by extension, ignoring case', () => {
    expect(categoryOf('Holiday.MP4', 'file')).toBe('video')
    expect(categoryOf('Outlook.pst', 'file')).toBe('document')
    expect(categoryOf('install.wim', 'file')).toBe('archive')
    expect(categoryOf('setup.exe', 'file')).toBe('program')
    expect(categoryOfExtension('PNG')).toBe('image')
  })

  it('treats folders and links as folders and unknown or missing extensions as other', () => {
    expect(categoryOf('Videos', 'dir')).toBe('folder')
    expect(categoryOf('Shortcut', 'link')).toBe('folder')
    expect(categoryOf('README', 'file')).toBe('other')
    expect(categoryOf('.gitignore', 'file')).toBe('other')
    expect(categoryOf('data.xyz', 'file')).toBe('other')
  })
})
