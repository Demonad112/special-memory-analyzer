<script setup lang="ts">
import NavRail from './components/NavRail.vue'
import TitleBar from './components/TitleBar.vue'
</script>

<template>
  <div class="flex h-full flex-col">
    <TitleBar />
    <div class="flex min-h-0 flex-1">
      <NavRail />
      <main
        class="min-w-0 flex-1 overflow-auto rounded-tl-lg border-t border-l border-stroke bg-layer"
      >
        <RouterView />
      </main>
    </div>
  </div>
</template>
