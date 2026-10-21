/** File-type groups for the treemap colours and the "File types" list. */
export type Category =
  'folder' | 'video' | 'audio' | 'image' | 'document' | 'archive' | 'program' | 'other'

export const categoryLabels: Record<Category, string> = {
  folder: 'Folders',
  video: 'Video',
  audio: 'Music and audio',
  image: 'Pictures',
  document: 'Documents and email',
  archive: 'Archives, disk images and backups',
  program: 'Programs and system files',
  other: 'Other files',
}

const groups: Record<Exclude<Category, 'folder' | 'other'>, string> = {
  video: 'mp4 mkv avi mov wmv m4v webm mpg mpeg vob ts mts m2ts flv 3gp',
  audio: 'mp3 wav flac aac m4a wma ogg opus aiff mid',
  image: 'jpg jpeg png gif bmp tif tiff heic heif webp raw cr2 cr3 nef arw dng psd svg ico',
  document:
    'pdf doc docx docm xls xlsx xlsm xlsb ppt pptx txt rtf csv odt ods odp md msg eml pst ost one log xml json',
  archive: 'zip rar 7z tar gz tgz bz2 xz zst cab iso img vhd vhdx vmdk wim esd bak tib',
  program: 'exe dll msi msix msp appx appxbundle sys drv ocx cpl mui cat etl dat pak',
}

const byExtension = new Map<string, Category>(
  Object.entries(groups).flatMap(([category, list]) =>
    list.split(' ').map((extension) => [extension, category as Category] as const),
  ),
)

/** The group for a lowercase extension without the dot ("mp4"). */
export function categoryOfExtension(extension: string): Category {
  return byExtension.get(extension.toLowerCase()) ?? 'other'
}

/** The group for a file or folder name. */
export function categoryOf(name: string, kind: 'file' | 'dir' | 'link'): Category {
  if (kind !== 'file') {
    return 'folder'
  }
  const dot = name.lastIndexOf('.')
  return dot > 0 ? categoryOfExtension(name.slice(dot + 1)) : 'other'
}
