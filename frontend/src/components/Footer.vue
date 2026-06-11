<template>
   <MDBFooter v-if="isAnySectionAvailable" bg="dark" text="light" class="fixed-bottom ra-footer border-top border-secondary-subtle">
      <MDBContainer fluid class="py-2">
         <MDBRow center class="g-2 align-items-center">
            <MDBCol auto v-for="section in visibleSections" :key="section.heading">
               <RouterLink class="ra-footer__link" :to="getLinkTo(section.heading)">
                  <span>{{ section.label }}</span>
                  <MDBBadge color="light" pill class="ra-footer__count ms-1 text-dark">
                     {{ section.count }}
                  </MDBBadge>
               </RouterLink>
            </MDBCol>
            <MDBCol auto v-if="hasFilter" class="ra-footer__filter">
               <MDBIcon icon="circle-exclamation" class="me-1" />
               <span>Filtered by <em>{{ appNameFilter }}</em></span>
            </MDBCol>
         </MDBRow>
      </MDBContainer>
   </MDBFooter>
</template>

<style scoped>
.ra-footer__link {
   align-items: center;
   border-radius: 0.5rem;
   color: inherit;
   display: inline-flex;
   padding: 0.35rem 0.5rem;
   text-decoration: none;
}

.ra-footer__link:hover,
.ra-footer__link:focus-visible {
   background-color: rgba(255, 255, 255, 0.12);
   color: inherit;
}

.ra-footer__count {
   min-width: 2rem;
   text-align: center;
}

.ra-footer__filter {
   font-size: 0.875rem;
   opacity: 0.95;
}
</style>

<script setup>
   import { computed } from 'vue';
   import { useStore } from 'vuex';
   import { useRoute } from "vue-router";
   import { MDBBadge, MDBCol, MDBContainer, MDBFooter, MDBIcon, MDBRow } from 'mdb-vue-ui-kit';

   const store = useStore();
   const route = useRoute();

   const appsWithTicket = computed(() => store.getters.appsWithTicket);
   const appsWithoutTicket = computed(() => store.getters.appsWithoutTicket);
   const appBackups = computed(() => store.getters.appBackups);
   const ownedApps = computed(() => store.getters.ownedApps);
   const appNameFilter = computed(() => store.getters.appNameFilter);
   const hasFilter = computed(() => appNameFilter.value != null && appNameFilter.value.trim() !== '');

   const sections = computed(() => {
      return [
         { heading: 'my-previews', label: 'My Previews', count: ownedApps.value.length },
         { heading: 'apps-without-tickets', label: 'Previews', count: appsWithoutTicket.value.length },
         { heading: 'apps-with-tickets', label: 'Features', count: appsWithTicket.value.length },
         { heading: 'backed-up-apps', label: 'Backups', count: appBackups.value.length }
      ];
   });

   const visibleSections = computed(() => sections.value.filter((section) => section.count > 0));

   const isAnySectionAvailable = computed(() => visibleSections.value.length > 0);

   function getLinkTo(heading) {
      return { query: route.query, params: { heading } };
   }
</script>
