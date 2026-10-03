<script setup lang="ts">
/**
 * The stand-in for a hit without a cover (RD-101-17, RD-190-16).
 *
 * No cover, no gap: without a stand-in the titles of a list start in two different places, which
 * is what makes a long list read as unruly. Decoration, so it is hidden from screen readers and
 * carries the application's own mark rather than a picture that pretends to be of the thing.
 *
 * One component for every list that draws covers — the subscription hits and the detailed rows
 * of the indexer search — so a hit without a picture looks the same wherever it stands. The
 * caller gives the size (`size-12`, `size-10`), the same as the cover it stands in for.
 */
</script>

<template>
  <span
    aria-hidden="true"
    data-testid="cover-placeholder"
    class="shrink-0 bg-elevated bg-[length:1.5rem] bg-center bg-no-repeat"
    style="background-image: url(/favicon.svg)"
  />
</template>
