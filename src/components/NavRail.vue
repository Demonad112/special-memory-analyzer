<script setup lang="ts">
import { useRoute } from 'vue-router'
import { sections, settingsSection, type Section } from '@/router'

const route = useRoute()

/** A section stays selected on its sub-pages (Compare → Record a copy → report). */
function isActive(section: Section): boolean {
  return (
    route.path === section.path ||
    (section.path !== '/' && route.path.startsWith(`${section.path}/`))
  )
}
</script>

<template>
  <nav
    class="flex w-56 shrink-0 flex-col gap-1 px-1.5 pb-2"
    aria-label="Sections"
  >
    <RouterLink
      v-for="section in [...sections, settingsSection]"
      :key="section.path"
      :to="section.path"
      class="relative flex h-9 items-center gap-3 rounded-md px-3 text-fg hover:bg-subtle"
      :class="{
        'mt-auto': section === settingsSection,
        'bg-subtle-strong is-active': isActive(section),
      }"
      :aria-current="isActive(section) ? 'page' : undefined"
      active-class=""
      exact-active-class=""
    >
      <component
        :is="section.icon"
        class="size-4"
        :stroke-width="1.75"
      />
      <span>{{ section.title }}</span>
    </RouterLink>
  </nav>
</template>

<style scoped>
/* Fluent selection pill on the active item. */
.is-active::before {
  position: absolute;
  top: 50%;
  left: 0;
  width: 3px;
  height: 16px;
  border-radius: 2px;
  background: var(--app-accent);
  content: '';
  transform: translateY(-50%);
}
</style>
