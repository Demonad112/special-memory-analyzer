<script setup lang="ts">
defineProps<{ title: string; summary?: string }>()
</script>

<template>
  <header class="mb-6">
    <h1 class="font-display text-[28px] leading-9 font-semibold">{{ title }}</h1>
    <p
      v-if="summary"
      class="mt-1 max-w-2xl text-muted"
    >
      {{ summary }}
    </p>
  </header>
</template>
