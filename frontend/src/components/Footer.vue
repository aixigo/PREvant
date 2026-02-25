<template>
   <footer class="footer fixed-bottom text-light bg-dark" v-if="isAnySectionAvailable">
      <div class="container-fluid text-center text-md-left">
         <div class="row">
            <div class="col-md-12">
               <div class="text-center py-3">
                  <RouterLink :to="getLinkTo('my-previews')" v-if="ownedApps.length > 0">
                     My Previews ({{ ownedApps.length }})
                  </RouterLink>
                  <RouterLink :to="getLinkTo('apps-without-tickets')" v-if="appsWithoutTicket.length > 0">
                     Previews ({{ appsWithoutTicket.length }})
                  </RouterLink>
                  <RouterLink :to="getLinkTo('apps-with-tickets')" v-if="appsWithTicket.length > 0">
                       Features ({{ appsWithTicket.length }})
                  </RouterLink>
                  <RouterLink :to="getLinkTo('backed-up-apps')" v-if="appBackups.length > 0">
                       Backups ({{ appBackups.length }})
                  </RouterLink>
                  <span v-if="appNameFilter != ''">
                     <font-awesome-icon icon="exclamation"/> Filtered by <em>{{ appNameFilter }}</em>
                  </span>
               </div>
            </div>
         </div>
      </div>
   </footer>
</template>

<style scoped>
footer a {
   padding: 0.5em;
}
</style>

<script setup>
   import { computed } from 'vue';
   import { useStore } from 'vuex';
   import { useRoute } from "vue-router";

   const store = useStore();
   const route = useRoute();

   const appsWithTicket = computed(() => store.getters.appsWithTicket);
   const appsWithoutTicket = computed(() => store.getters.appsWithoutTicket);
   const appBackups = computed(() => store.getters.appBackups);
   const ownedApps = computed(() => store.getters.ownedApps);
   const appNameFilter = computed(() => store.getters.appNameFilter);

   const isAnySectionAvailable = computed(() => {
      return store.getters.ownedApps.length > 0 || store.getters.appsWithTicket.length > 0 || store.getters.appsWithoutTicket.length > 0 || store.getters.appBackups.length > 0;
   })

   function getLinkTo(heading) {
      return { query: route.query, params: { heading } };
   }
</script>
