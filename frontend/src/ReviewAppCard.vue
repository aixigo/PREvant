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
                  <button class="btn bmd-btn-icon dropdown-toggle" type="button" :id="'menu' + reviewApp.name"
                          data-toggle="dropdown"
                          aria-haspopup="true" aria-expanded="false">
                     <i class="material-icons">more_vert</i>
                  </button>

                  <div class="dropdown-menu dropdown-menu-left" :aria-labelledby="'menu' + reviewApp.name">
                     <button type="button" class="dropdown-item btn btn-primary" @click="copyVersions()">
                        <font-awesome-icon icon="clipboard"/> &nbsp; Versions
                     </button>
                     <button type="button" class="dropdown-item btn btn-primary" @click="duplicateApp()">
                        <font-awesome-icon icon="copy"/> &nbsp; Duplicate
                     </button>
                     <button type="button" class="dropdown-item btn btn-danger" @click="openDeleteDialog()"
                             v-if="reviewApp.name != 'master'">
                        <font-awesome-icon icon="trash"/> &nbsp; Shutdown
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
                 :key="container.name"
                 class="ra-container">

               <div class="ra-container__type"
                    :class="{ 'is-expanded': isExpanded( container ) }"
                    @click="toggleContainer( container )">

                  <template>
                     <i v-if="isExpanded( container )" class="ra-icon--expander ra-icons material-icons">keyboard_arrow_down</i>
                     <i v-else="!isExpanded( container )" class="ra-icon--expander ra-icons material-icons">keyboard_arrow_right</i>
                  </template>
                  <template>
                     <i v-if="container.name.endsWith( 'openid' )" class="ra-icons  material-icons">security</i>
                     <i v-else-if="container.name.endsWith( '-proxy' )" class="ra-icons  material-icons">call_split</i>
                     <i v-else-if="container.name.endsWith( '-frontend' )" class="ra-icons  material-icons">web</i>
                     <i v-else-if="container.name.endsWith( '-service' )" class="ra-icons  material-icons">dns</i>
                     <i v-else-if="container.name.endsWith( '-api' )" class="ra-icons  material-icons">developer_board</i>
                     <i v-else-if="container.name.endsWith( '-db' ) || container.name.endsWith( '-database' )" class="ra-icons  material-icons">archive</i>
                     <i v-else class="ra-icons  material-icons">link</i>
                  </template>
               </div>

               <div class="ra-container__infos">
                  <h5>
                     <a v-if="container.url" :href='container.url' target="_blank">{{ container.name }}</a>
                     <span v-else>{{ container.name }}</span>
                  </h5>

                  <div class="ra-build-infos__wrapper"
                       v-if="isExpanded( container )">
                     <div class="ra-build-infos">
                        <a v-if="container.openApiUrl" href="#" @click="currentApiUrl = container.openApiUrl">API
                           Documentation</a>

                        <a href="#" @click="showLogs($event, container.name)">Logs</a>
                     </div>

                     <div v-if="container.version && container.version.dateModified" class="ra-build-infos">
                        <span>{{ container.version.dateModified | date }}</span>,
                        <span>{{ container.version.dateModified | time }}</span>
                     </div>
                  </div>
               </div>

               <div class="ra-container__tags">
                  <span class="badge"
                        :class="badgeClass( container.type )"
                        v-tooltip="tooltip( container.type )">{{ container.type }}</span>
                  <span v-if="container.version && container.version.gitCommit"
                        class="ra-build-infos ra-build-infos__hash text-right"
                        :title="container.version.gitCommit">
                     {{ container.version.gitCommit.slice( 0, 7 ) }}…
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

      <open-api-ui :url="currentApiUrl" v-if="currentApiUrl != null" @close="currentApiUrl = null"/>

      <duplicate-app-dialog ref="duplicateDlg" :duplicate-from-app-name="reviewApp.name"/>
   </div>
</template>

<script>
   import moment from 'moment';
   import DuplicateAppDialog from './DuplicateAppDialog.vue';
   import ShutdownAppDialog from './ShutdownAppDialog.vue';
   import OpenApiUI from './OpenApiUI.vue';

   export default {
      data() {
         return {
            currentApiUrl: null,
            currentAppName: window.location.hash.slice(1),
            expandedContainers: {}
         };
      },
      components: {
         'duplicate-app-dialog': DuplicateAppDialog,
         'shutdown-app-dialog': ShutdownAppDialog,
         'open-api-ui': OpenApiUI
      },
      filters: {
         date(buildDateTime) {
            if (buildDateTime == null) {
               return 'N/A';
            }

            const date = moment(buildDateTime);
            if (date.isValid()) {
               return date.toDate().toLocaleDateString()
            }
            return buildDateTime;
         },
         time(buildDateTime) {
            if (buildDateTime == null) {
               return 'N/A';
            }

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
      watch: {
         reviewApp: function (newValue) {
            const {containers} = newValue;

            if (containers && containers.length) {
               containers.forEach(({name, version, openApiUrl}) => {
                  const expanded = version != null || openApiUrl != null;
                  this.$set(this.expandedContainers, [name], expanded);
               });
            }
         }
      },
      computed: {
         containerVersions() {
            const res = {};

            if (this.reviewApp.containers !== undefined) {
               this.reviewApp.containers
                  .filter(container => !!container.version)
                  .forEach(container => {
                     res[container.name] = container.version.gitCommit;
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
         duplicateApp() {
            this.$refs.duplicateDlg.open();
         },
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
         badgeClass(serviceType) {
            switch (serviceType) {
               case 'instance':
                  return 'badge-info';
               case 'linked':
                  return 'badge-warning';
               case 'replica':
                  return 'badge-dark';
            }
            return 'badge-secondary';
         },
         tooltip(serviceType) {
            switch (serviceType) {
               case 'instance':
                  return 'This service has been deployed especially for the review-app.';
               case 'replica':
                  return 'This service has been replicated from the service of the master review app. Changes to this service won\'t affect the service of master review-app.';
            }
            return '';
         },
         toggleContainer(container) {
            this.$set(this.expandedContainers, [container.name], !this.isExpanded(container));
         },
         isExpanded(container) {
            if (this.expandedContainers[container.name] == undefined) {
               return container.openApiUrl != null;
            }

            return this.expandedContainers[container.name] == true;
         },
         showLogs(event, service) {
            this.$emit('showLogs', this.reviewApp.name, service)
         }
      }
   }

   function latestBuildTime(app) {
      const max = (a, b) => a >= b ? a : b;
      return app.containers
         .filter(({version}) => !!version && !!version.dateModified)
         .map(({version}) => version.dateModified)
         .reduce(max, 0);
   };
</script>
