/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 aixigo AG
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
   <div>
      <div class="card" :class="backgroundClass" :id="reviewApp.name">
         <div class="card-header">
            <div class="d-flex justify-content-between">
               <h4 v-if="reviewApp.ticket !== undefined"
                  class="ra-headline">
                  <a :href="reviewApp.ticket.link" target="_blank">{{ reviewApp.name }}</a>
               </h4>
               <h4 v-else>
                  {{ reviewApp.name }}
               </h4>

               <div class="dropdown menu">
                  <button class="btn bmd-btn-icon dropdown-toggle" type="button" :id="'menu' + reviewApp.name" data-toggle="dropdown"
                        aria-haspopup="true" aria-expanded="false">
                     <i class="material-icons">more_vert</i>
                  </button>

                  <div class="dropdown-menu dropdown-menu-left" :aria-labelledby="'menu' + reviewApp.name">
                     <button type="button" class="dropdown-item btn btn-primary" @click="copyVersions()">
                        Copy Versions
                     </button>
                     <button type="button" class="dropdown-item btn btn-danger" @click="openDeleteDialog()"
                           v-if="reviewApp.name != 'master'">
                        Shutdown
                     </button>
                  </div>
               </div>
            </div>

            <div v-if="reviewApp.ticket !== undefined"
               class="ra-headline__intro">
               <span class="ra-ellipsis"
                  :title="reviewApp.ticket['summary']">{{ reviewApp.ticket['summary'] }}</span>
               <span class="badge"
                  :class="{ 'jira--ready': reviewApp.ticket['status'] === 'Bereit',
                            'jira--process': reviewApp.ticket['status'] === 'In Bearbeitung',
                            'jira--review': reviewApp.ticket['status'] === 'Review',
                            'jira--done': reviewApp.ticket['status'] === 'Erledigt' }">
                  {{ reviewApp.ticket['status'] }}
               </span>
            </div>
         </div>

         <div class="card-body">
            <div v-for="container in reviewApp.containers"
                 :key="container.vhost"
                 class="ra-container">
               <div class="ra-container__type">
                  <i v-if="container.vhost.endsWith( '-frontend' )" class="material-icons">web</i>
                  <i v-if="container.vhost.endsWith( '-service' )" class="material-icons">dns</i>
                  <i v-if="container.vhost.endsWith( '-api' )" class="material-icons">developer_board</i>
               </div>

               <div class="ra-container__infos">
                  <h5>
                     <a :href='container.url' target="_blank">{{ container.vhost }}</a>
                  </h5>
                  <div  class="ra-build-infos">
                     <a v-if="container.swaggerUrl" :href="container.swaggerUrl" target="_blank">API Documentation</a>
                     <a v-if="container.logsUrl" :href="container.logsUrl" target="_blank">Logs</a>
                  </div>

                  <div v-if="container.version" class="ra-build-infos">
                     <span class="ra-build-infos__date">{{ container.version[ 'build.time' ] | date }}</span>,
                     <span class="ra-build-infos__time">{{ container.version[ 'build.time' ] | time }}</span>
                     <!-- only for layout -->
                     <!-- <span class="ra-build-infos__date">20.07.2018</span>,
                     <span class="ra-build-infos__time">07:54</span> -->
                  </div>
                  <p v-if="!container.version && container.vhost.endsWith( '-service' )">
                     <font-awesome-icon icon="spinner" spin />
                  </p>
               </div>

               <div class="ra-container__tags">
                  <span class="badge" :class="badgeClass( container.containerType )" v-tooltip="tooltip( container.containerType )">{{ container.containerType }}</span>
                  <span v-if="container.version && container.version[ 'git.revision' ]"
                     class="ra-build-infos ra-build-infos__hash text-right"
                     :title="container.version[ 'git.revision' ]">
                     {{ container.version[ 'git.revision' ].slice( 0, 7 ) }}…
                     <!-- only for layout -->
                     <!-- c63ae57… -->
                  </span>
               </div>
            </div>

            <textarea
               v-if="displayVersion"
               class="ra-version-display"
               ref="versionDisplay"
               :value="displayVersion"
               maxlength="500"
               autocomplete="off"
               autocorrect="off"
               autocapitalize="off"
               spellcheck="false">
            </textarea>
         </div>
      </div>

      <shutdown-app-dialog ref="deleteDlg" :app-name="reviewApp.name" v-if="reviewApp.name != 'master'"/>
   </div>
</template>

<script>
   import moment from 'moment';
   import ShutdownAppDialog from './ShutdownAppDialog.vue';

   export default {
      data() {
         return {
            currentAppName: window.location.hash.slice(1)
         };
      },
      components: {
         'shutdown-app-dialog': ShutdownAppDialog
      },
      filters: {
         date(buildDateTime) {
            const date = moment(buildDateTime);
            if (date.isValid()) {
               return date.toDate().toLocaleDateString()
            }
            return buildDateTime;
         },
         time(buildDateTime) {
            const date = moment(buildDateTime);
            if (date.isValid()) {
               return date.toDate().toLocaleTimeString()
            }
            return buildDateTime;
         }
      },
      props: {
         reviewApp: {type: Object}
      },
      computed: {
         containerVersions() {
            const res = {};

            if (this.reviewApp.containers !== undefined) {
               this.reviewApp.containers
                  .filter(container => !!container.version)
                  .forEach(container => {
                     res[container.vhost] = container.version['git.revision'];
                  });
            }

            return res;
         },
         displayVersion() {
            const versions =
               Object.entries(this.containerVersions).map(([k, v]) => `${k}=${v}`).join(', ');
            return `[${this.reviewApp.name}@${latestBuildTime(this.reviewApp)}; ${versions}]`;
         },
         backgroundClass() {
            if (this.currentAppName === this.reviewApp.name) {
               return 'bg-emphasize'
            }

            return '';
         }
      },
      methods: {
         copyVersions() {
            const {versionDisplay} = this.$refs;
            versionDisplay.focus();
            versionDisplay.select();
            try {
               const success = document.execCommand('copy');
               if (!success) {
                  return;
               }
               versionDisplay.blur();
               setTimeout(() => {
                  document.body.focus();
               }, 100);
            }
            catch (err) { /* no browser support: text stays selected, can copy manually */
            }
         },
         openDeleteDialog() {
            this.$refs.deleteDlg.open();
         },
         badgeClass( containerType ) {
            switch ( containerType ) {
               case 'instance':
                  return 'badge-info';
               case 'linked':
                  return 'badge-warning';
               case 'replica':
                  return 'badge-dark';
            }
            return 'badge-secondary';
         },
         tooltip( containerType ) {
            switch ( containerType ) {
               case 'instance':
                  return 'This service has been deployed especially for the review-app.';
               case 'linked':
                  return 'This service has been linked to the service of the master review-app. Every change to this service affects the service of master review-app.';
               case 'replica':
                  return 'This service has been replicated from the service of the master review app. Changes to this service won\'t affect the service of master review-app.';
            }
            return '';
         },
      }
   }

   function latestBuildTime(app) {
      const max = (a, b) => a >= b ? a : b;
      return app.containers
         .filter(({version}) => !!version)
         .map(({version}) => version['build.time'])
         .reduce(max, 0);
   };
</script>
