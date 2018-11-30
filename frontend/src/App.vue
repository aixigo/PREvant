<template>
   <div class="container" id="app">

      <h1 class="ra-container__title--preview">Previews</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--preview ra-apps ">
         <review-app-card
            v-for="reviewApp in appsWithoutTicket(reviewApps)"
            :key="reviewApp.name"
            :review-app="reviewApp"
            class="list-complete-item"/>
      </transition-group>

      <h1 class="ra-container__title--feature">Features</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--feature ra-apps">
         <review-app-card
            v-for="reviewApp in appsWithTicket(reviewApps)"
            :key="reviewApp.name"
            :review-app="reviewApp"
            class="list-complete-item"/>
      </transition-group>
   </div>
</template>

<style>
   .list-complete-item {
      transition: all 1s;
   }

   .list-complete-enter, .list-complete-leave-to
      /* .list-complete-leave-active below version 2.1.8 */
   {
      opacity: 0;
      transform: translateY(30px);
   }

   .list-complete-leave-active {
      position: absolute;
   }
</style>

<script>
   import { mapGetters } from 'vuex';
   import ReviewAppCard from './ReviewAppCard.vue';

   export default {
      data() {
         return {};
      },
      components: {
         'review-app-card': ReviewAppCard
      },
      computed: {
         ...mapGetters( [ 'reviewApps' ] )
      },
      methods: {
         appsWithTicket( apps ) {
            const self = this;
            return apps.filter( app => self.$store.state.tickets[ app.name ] !== undefined );
         },

         appsWithoutTicket( apps ) {
            const self = this;
            return apps.filter( app => self.$store.state.tickets[ app.name ] === undefined );
         }
      }
   };


</script>
