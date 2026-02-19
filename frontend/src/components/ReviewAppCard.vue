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
      <div class="card">
         <div class="card-header">
            <div class="d-flex justify-content-between">
               <h4 v-if="reviewApp.ticket !== undefined"
                   class="ra-headline ra-app-title">
                  <a :href="reviewApp.ticket.link" target="_blank">{{ reviewApp.name }}</a>
                  <span v-if="reviewApp.status === 'backed-up'" class="badge badge-backed-up ml-2">Backed up</span>
               </h4>
               <h4 v-else class="ra-app-title">
                  {{ reviewApp.name }}
                  <span v-if="reviewApp.status === 'backed-up'" class="badge badge-backed-up ml-2">Backed up</span>
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
                     <button type="button" class="dropdown-item btn btn-primary" @click="openBackupDialog()">
                        <template v-if="reviewApp.status === 'backed-up'">
                           <font-awesome-icon icon="server"/> &nbsp; Redeploy
                        </template>
                        <template v-else>
                           <font-awesome-icon icon="download"/> &nbsp; Back up
                        </template>
                     </button>
                     <button type="button" class="dropdown-item btn btn-danger" @click="openDeleteDialog()"
                             v-if="reviewApp.name !== defaultAppName">
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
                 class="ra-container"
                 :class="{ 'ra-container__paused': container.status !== 'running' }">

               <div class="ra-container__type"
                    :class="{ 'is-expanded': isExpanded( container ) }"
                    @click="toggleContainer( container )">

                  <i v-if="isExpanded( container )" class="ra-icon--expander ra-icons material-icons">keyboard_arrow_down</i>
                  <i v-else="!isExpanded( container )" class="ra-icon--expander ra-icons material-icons">keyboard_arrow_right</i>

                  <i v-if="container.name.endsWith( 'openid' )" class="ra-icons  material-icons">security</i>
                  <i v-else-if="container.name.endsWith( '-proxy' )" class="ra-icons  material-icons">call_split</i>
                  <i v-else-if="container.name.endsWith( '-frontend' )" class="ra-icons  material-icons">web</i>
                  <i v-else-if="container.name.endsWith( '-service' )" class="ra-icons  material-icons">dns</i>
                  <i v-else-if="container.name.endsWith( '-api' )" class="ra-icons  material-icons">developer_board</i>
                  <i v-else-if="container.name.endsWith( '-db' ) || container.name.endsWith( '-database' )" class="ra-icons  material-icons">archive</i>
                  <i v-else class="ra-icons  material-icons">link</i>
               </div>

               <div class="ra-container__infos">
                  <h5>
                     <a v-if="container.url" :href='container.url' target="_blank">{{ container.name }}</a>
                     <span v-else>{{ container.name }}</span>

                     <button type="button" class="btn btn-dark ra-container__change-status" @click="changeState($event, container.name)" v-if="reviewApp.status == 'deployed'">
                        <template v-if="container.status === 'running'">
                           <i class="ra-icons  material-icons">pause_circle_outline</i>
                        </template>
                        <template v-else>
                           <i class="ra-icons  material-icons">play_circle_outline</i>
                        </template>
                     </button>
                  </h5>

                  <div class="ra-build-infos__wrapper"
                       v-if="isExpanded( container )">
                     <div class="ra-build-infos">
                        <router-link v-if="container.openApiUrl" :to="{ name: 'open-api-ui', params: {  url: container.openApiUrl }, meta: { title: container.name }}">Open API Documentation</router-link>
                        <router-link v-if="container.asyncApiUrl" :to="{ name: 'async-api-ui', params: { url: container.asyncApiUrl }, meta: { title: container.name }}">Async API Documentation</router-link>
                        <router-link :to="{ name: 'logs', params: {  app: reviewApp.name, service: container.name }}">Logs</router-link>
                     </div>

                     <div v-if="container.version && container.version.dateModified" class="ra-build-infos">
                        <span>{{ formatBuildDate( container.version.dateModified ) }}</span>,
                        <span>{{ formatBuildTime( container.version.dateModified ) }}</span>
                     </div>
                  </div>
               </div>

               <div class="ra-container__tags">
                  <span class="badge"
                        :class="badgeClass( container.type )">{{ container.type }}</span>
                  <span v-if="container.version && container.version.gitCommit"
                        class="ra-build-infos ra-build-infos__hash text-right"
                        :title="formatVersion( container.version )">
                     {{ formatSlicedVersion( container.version ) }}
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
                  :spellcheck="false">
            </textarea>
         </div>

         <template v-if="showOwners">
            <div class="card-footer text-muted" v-if="reviewApp.owners == null || reviewApp.owners.length === 0">
               No known owners
            </div>
            <div class="owners card-footer text-muted" v-else>
               Owners:
               <span class="badge badge-secondary" v-for="owner in reviewApp.owners">
                  <template v-if="owner.name">{{ owner.name }}</template>
                  <template v-else>{{ owner.sub }}</template>
               </span>
            </div>
         </template>
      </div>

      <shutdown-app-dialog ref="deleteDlg" :app-name="reviewApp.name" v-if="reviewApp.name !== defaultAppName"/>
      <duplicate-app-dialog ref="duplicateDlg" :duplicate-from-app-name="reviewApp.name"/>
      <backup-app-dialog ref="backupDlg" :app-name="reviewApp.name" :app-status="reviewApp.status"/>
   </div>
</template>

<style lang="css" scoped>
.owners {
   overflow: hidden;
   white-space: nowrap;
}
.owners span {
    margin: 0 0.2em;
}
.ra-app-title {
   display: flex;
   align-items: center;
}
.badge-backed-up {
   background-color: #ef6c00;
   color: #fff;
}
</style>

<script>
   import moment from 'moment';
   import BackupAppDialog from './BackupAppDialog.vue';
   import DuplicateAppDialog from './DuplicateAppDialog.vue';
   import ShutdownAppDialog from './ShutdownAppDialog.vue';
   import { useConfig } from '../composables/useConfig';

   export default {
      setup() {
         const { defaultAppName } = useConfig();

         return {
            defaultAppName
         };
      },
      data() {
         return {
            expandedContainers: {}
         };
      },
      components: {
         'backup-app-dialog': BackupAppDialog,
         'duplicate-app-dialog': DuplicateAppDialog,
         'shutdown-app-dialog': ShutdownAppDialog,
      },
      props: {
         reviewApp: {type: Object},
         showOwners: {type: Boolean}
      },
      watch: {
         reviewApp: function (newValue) {
            const {containers} = newValue;

            if (containers && containers.length) {
               containers.forEach(({name, version, openApiUrl}) => {
                  const expanded = version != null || openApiUrl != null;
                  this.expandedContainers[name] = expanded;
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
                     res[container.name] = this.formatVersion(container.version);
                  });
            }

            return res;
         },
         displayVersion() {
            const versions =
               Object.entries(this.containerVersions).map(([k, v]) => `${k}=${v}`).join(', ');
            return `[${this.reviewApp.name}@${latestBuildTime(this.reviewApp)}; ${versions}]`;
         },
      },
      methods: {
         duplicateApp() {
            this.$refs.duplicateDlg.open();
         },
         openBackupDialog() {
            this.$refs.backupDlg.open();
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
         toggleContainer(container) {
            this.expandedContainers[container.name] = !this.isExpanded(container);
         },
         isExpanded(container) {
            if (this.expandedContainers[container.name] == undefined) {
               return container.openApiUrl != null || container.asyncApiUrl != null;
            }

            return this.expandedContainers[container.name] == true;
         },
         changeState(event, service) {
            this.$emit('changeState', this.reviewApp.name, service);
         },

         formatBuildDate(buildDateTime) {
            if (buildDateTime == null) {
               return 'N/A';
            }

            const date = moment(buildDateTime);
            if (date.isValid()) {
               return date.toDate().toLocaleDateString()
            }
            return buildDateTime;
         },
         formatBuildTime(buildDateTime) {
            if (buildDateTime == null) {
               return 'N/A';
            }

            const date = moment(buildDateTime);
            if (date.isValid()) {
               return date.toDate().toLocaleTimeString()
            }
            return buildDateTime;
         },

         formatVersion(version) {
            if (version.softwareVersion != null) {
               if (version.gitCommit != null) {
                  return `${version.softwareVersion} (Commit: ${version.gitCommit})`;
               } else {
                  return version.softwareVersion;
               }
            }

            if (version.gitCommit != null) {
               return version.gitCommit;
            }

            return '';
         },


         formatSlicedVersion(version) {
            if (version.softwareVersion != null) {
               return version.softwareVersion.slice(0, 16);
            }

            if (version.gitCommit != null) {
               return version.gitCommit.slice(0, 7);
            }

            return '';
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
