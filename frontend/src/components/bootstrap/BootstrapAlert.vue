<template>
    <div class="alert text-center" :class="`alert-${props.type}`" role="alert">
        <slot />
    </div>
</template>

<script setup>
// The Alert component from MDB requires the PRO package.
// This is why we create our own wrapper leveraging the bootstrap classes.

const props = defineProps({
    type: {
        type: String,
        default: 'primary'
    }
})
</script>
