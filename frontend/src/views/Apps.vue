/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */

<template>
   <div class="container" id="app">

      <spinner v-if="isFetchInProgress" />

      <div v-if="errors.length > 0" class="alert alert-danger" role="alert">
         <p v-for="error in errors">
            <b>{{ error.title }}</b>: {{ error.detail }}
         </p>
      </div>
      <h1 v-else-if="reviewApps.length === 0" class="ra-container__title--preview">
         No apps to review.
      </h1>

      <h1 id="my-previews" class="ra-container__title ra-container__title--preview" v-if="ownedApps.length > 0">My Previews</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--preview ra-apps ">
         <review-app-card
            v-for="reviewApp in ownedApps"
            :key="reviewApp.name"
            :review-app="reviewApp"
            :showOwners="issuers != null"
            v-on:changeState="changeServiceState"
            class="list-complete-item"/>
      </transition-group>

      <h1 id="apps-without-tickets" class="ra-container__title ra-container__title--feature" v-if="appsWithoutTicket.length > 0">Previews</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--preview ra-apps ">
         <review-app-card
            v-for="reviewApp in appsWithoutTicket"
            :key="reviewApp.name"
            :review-app="reviewApp"
            :showOwners="issuers != null"
            v-on:changeState="changeServiceState"
            class="list-complete-item"/>
      </transition-group>

      <h1 id="apps-with-tickets" class="ra-container__title ra-container__title--feature" v-if="appsWithTicket.length > 0">Features</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--feature ra-apps">
         <review-app-card
            v-for="reviewApp in appsWithTicket"
            :key="reviewApp.name"
            :review-app="reviewApp"
            :showOwners="issuers != null"
            v-on:changeState="changeServiceState"
            class="list-complete-item"/>
      </transition-group>

      <h1 id="backed-up-apps" class="ra-container__title ra-container__title--feature" v-if="appBackups.length > 0">Backups</h1>
      <transition-group tag="div" name="list-complete" class="ra-container__apps--feature ra-apps">
         <review-app-card
            v-for="reviewApp in appBackups"
            :key="reviewApp.name"
            :review-app="reviewApp"
            :showOwners="issuers != null"
            v-on:changeState="changeServiceState"
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

   .alert > p {
      margin-bottom: 0;
      text-align: center;
   }
   .alert > p + p {
      margin-top: 1rem;
   }
</style>

<script>
   import { mapGetters } from 'vuex';
   import ReviewAppCard from '../components/ReviewAppCard.vue';
   import Spinner from '../components/Spinner.vue';

   export default {
      data() {
         return {
            scrolledTo: null
         };
      },
      components: {
         'review-app-card': ReviewAppCard,
         'spinner': Spinner
      },
      computed: {
         ...mapGetters([
            'issuers',
            'reviewApps',
            'appsWithTicket',
            'appsWithoutTicket',
            'appBackups',
            'ownedApps',
            'errors',
            'isFetchInProgress'
         ])
      },
      methods: {
         changeServiceState( appName, serviceName ) {
            this.$store.dispatch( 'changeServiceState', { appName, serviceName } );
         },
         async scrollToSection(section) {
            await this.$nextTick();
            const element = document.getElementById(section);
            if (element) {
               this.scrolledTo = section;
               element.scrollIntoView({ behavior: 'smooth' });
            }
         }
      },
      watch: {
         // TODO: this watcher is here instead of https://router.vuejs.org/guide/advanced/scroll-behavior
         // because with scroll-behavior there was currently no way of waiting for the reviewApps to be
         // available for the first time. When the page gets refreshed, it takes some time to load the applications
         // from the backend so the scroll position cannot be restored immediately because the selected section
         // is not yet available.
         reviewApps: {
            async handler(apps) {
               const heading = this.$route.params.heading;
               if (!heading) {
                  return;
               }

               if (this.scrolledTo == null && apps.length > 0) {
                  await this.scrollToSection(heading);
               }
            },
         },
         async $route (to) {
            await this.scrollToSection(to.params.heading);
         }
      }
   };


</script>
