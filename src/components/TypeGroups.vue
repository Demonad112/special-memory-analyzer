<script setup lang="ts">
import { computed } from 'vue'
import { shownBytes, type SizeMode, type TypeRow } from '@/lib/cleanup'
import { categoryLabels, categoryOfExtension, type Category } from '@/lib/fileTypes'
import { formatBytes, percentShare, plural } from '@/lib/format'

const props = defineProps<{ fileTypes: TypeRow[]; rootSize: number; sizeMode: SizeMode }>()

interface TypeGroup {
  category: Category
  size: number
  files: number
}
const typeGroups = computed<TypeGroup[]>(() => {
  const groups = new Map<Category, TypeGroup>()
  for (const type of props.fileTypes) {
    const category = categoryOfExtension(type.extension)
    const group = groups.get(category) ?? { category, size: 0, files: 0 }
    group.size += shownBytes(type, props.sizeMode)
    group.files += type.files
    groups.set(category, group)
  }
  return [...groups.values()].sort((a, b) => b.size - a.size)
})
</script>

<template>
  <div
    id="cleanup-panel-types"
    class="mt-2 grid gap-3 lg:grid-cols-2"
  >
    <ul
      class="rounded-lg border border-stroke bg-card p-3"
      aria-label="Space by type"
    >
      <li
        v-for="group in typeGroups"
        :key="group.category"
        class="py-1.5 text-[13px]"
      >
        <div class="flex items-center gap-2">
          <span
            class="size-3 shrink-0 rounded-sm"
            :style="{ background: `var(--app-type-${group.category})` }"
          />
          <span class="min-w-0 flex-1 truncate">{{ categoryLabels[group.category] }}</span>
          <span class="text-xs text-muted tabular-nums">{{ plural(group.files, 'file') }}</span>
          <span class="w-20 text-right tabular-nums">{{ formatBytes(group.size) }}</span>
        </div>
        <div class="mt-1 ml-5 h-1 overflow-hidden rounded-full bg-subtle-strong">
          <div
            class="h-full rounded-full"
            :style="{
              width: percentShare(group.size, rootSize),
              background: `var(--app-type-${group.category})`,
            }"
          />
        </div>
      </li>
    </ul>
    <ul
      class="max-h-96 overflow-y-auto rounded-lg border border-stroke bg-card"
      aria-label="Space by extension"
    >
      <li
        v-for="type in fileTypes.slice(0, 100)"
        :key="type.extension"
        class="flex items-center gap-2 border-b border-stroke px-3 py-1.5 text-[13px] last:border-b-0"
      >
        <span
          class="size-2.5 shrink-0 rounded-sm"
          :style="{ background: `var(--app-type-${categoryOfExtension(type.extension)})` }"
        />
        <span class="min-w-0 flex-1 truncate">
          {{ type.extension ? `.${type.extension}` : 'No extension' }}
        </span>
        <span class="text-xs text-muted tabular-nums">{{ plural(type.files, 'file') }}</span>
        <span class="w-11 text-right text-xs text-muted tabular-nums">
          {{ percentShare(shownBytes(type, sizeMode), rootSize) }}
        </span>
        <span class="w-20 text-right tabular-nums">
          {{ formatBytes(shownBytes(type, sizeMode)) }}
        </span>
      </li>
    </ul>
  </div>
</template>
