<script setup lang="ts">
import type { Tone } from '@/lib/diffRows'

defineProps<{ text: string; tone: Tone }>()

const toneClass: Record<Tone, string> = {
  success: 'text-success',
  warning: 'text-warning bg-subtle',
  danger: 'text-danger bg-danger-bg font-semibold',
  neutral: 'text-muted bg-subtle',
}
</script>

<template>
  <span
    class="inline-flex h-5 max-w-full items-center truncate rounded px-1.5 text-xs"
    :class="toneClass[tone]"
  >
    {{ text }}
  </span>
</template>
