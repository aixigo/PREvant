<template>
   <div class="ra-owners-display">
      <template v-if="hasOwners">
         <span class="ra-owners-display__label">Owners:</span>
         <span class="ra-owners-display__main-owner">{{ firstOwnerName }}</span>
         <MDBTooltip
            v-if="additionalOwnerCount > 0"
            v-model="isOwnersTooltipVisible"
            class="ra-owners-display__tooltip"
            direction="top"
            :max-width="320"
            tabindex="0"
         >
            <template #reference>
               <span class="ra-owners-display__tooltip-trigger">
                  <MDBBadge color="dark" class="ra-owners-display__badge">
                     +{{ additionalOwnerCount }}
                  </MDBBadge>
               </span>
            </template>
            <template #tip>
               <ul class="ra-owners-display__tooltip-list">
                  <li v-for="owner in remainingOwnerNames" :key="owner">{{ owner }}</li>
               </ul>
            </template>
         </MDBTooltip>
      </template>
      <span v-else>No known owners</span>
   </div>
</template>

<script setup>
   import { computed, ref } from 'vue';
   import { MDBBadge, MDBTooltip } from 'mdb-vue-ui-kit';

   const props = defineProps({
      owners: {
         type: Array,
         default: () => [],
      },
   });

   const isOwnersTooltipVisible = ref(false);

   const ownerNames = computed(() => {
      return props.owners
         .filter(owner => owner != null)
         .map((owner) => owner.name ?? owner.sub ?? '')
         .filter(ownerName => ownerName != null)
   })

   const hasOwners = computed(() => ownerNames.value.length > 0);
   const firstOwnerName = computed(() => ownerNames.value[0] ?? null);

   const additionalOwnerCount = computed(() => {
      return Math.max(ownerNames.value.length - 1, 0);
   });

   const remainingOwnerNames = computed(() => {
      return ownerNames.value.slice(1);
   });
</script>

<style scoped>
.ra-owners-display {
   display: flex;
   align-items: center;
   gap: 0.35rem;
   white-space: nowrap;
}

.ra-owners-display__label {
   font-weight: 500;
}

.ra-owners-display__main-owner {
   max-width: 12rem;
   overflow: hidden;
   text-overflow: ellipsis;
}

.ra-owners-display__tooltip-trigger {
   cursor: help;
   display: flex;
   align-items: center;
}

.ra-owners-display__badge {
   margin: 0;
   color: #fff;
   background-color: #6c757d;
}

.ra-owners-display__tooltip-list {
   margin: 0;
   padding-left: 1rem;
   white-space: normal;
   text-align: left;
}

.ra-owners-display__tooltip-list li {
   text-align: left;
}
</style>
