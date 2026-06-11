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
      <MDBCard class="h-100">
         <MDBCardHeader>
            <div class="d-flex justify-content-between align-items-center">
               <h4 v-if="reviewApp.ticket !== undefined"
                   class="ra-headline ra-app-title">
                  <a :href="reviewApp.ticket.link" target="_blank">{{ reviewApp.name }}</a>
                  <MDBBadge v-if="reviewApp.status === 'backed-up'" class="badge-backed-up ms-2">Backed up</MDBBadge>
               </h4>
               <h4 v-else class="ra-app-title">
                  {{ reviewApp.name }}
                  <MDBBadge v-if="reviewApp.status === 'backed-up'" class="badge-backed-up ms-2">Backed up</MDBBadge>
               </h4>

               <MDBDropdown v-model="menuOpen">
                  <MDBDropdownToggle
                     tag="a"
                     class="ra-menu-toggle p-0 border-0 bg-transparent dropdown-toggle"
                     :aria-label="`Open actions for ${reviewApp.name}`"
                     @click="menuOpen = !menuOpen">
                     <MDBIcon icon="ellipsis-vertical" />
                  </MDBDropdownToggle>

                  <MDBDropdownMenu>
                     <MDBDropdownItem tag="button" @click="copyVersions">
                        <MDBIcon icon="clipboard" /> &nbsp; Versions
                     </MDBDropdownItem>
                     <MDBDropdownItem tag="button" @click="duplicateApp">
                        <MDBIcon icon="copy" /> &nbsp; Duplicate
                     </MDBDropdownItem>
                     <MDBDropdownItem v-if="isBackupsEnabled" tag="button" @click="openBackupDialog">
                        <template v-if="reviewApp.status === 'backed-up'">
                           <MDBIcon icon="server" /> &nbsp; Redeploy
                        </template>
                        <template v-else>
                           <MDBIcon icon="download" /> &nbsp; Back up
                        </template>
                     </MDBDropdownItem>
                     <MDBDropdownItem
                        v-if="reviewApp.name !== defaultAppName"
                        tag="button"
                        class="text-danger"
                        @click="openDeleteDialog">
                        <MDBIcon icon="trash" /> &nbsp; Shutdown
                     </MDBDropdownItem>
                  </MDBDropdownMenu>
               </MDBDropdown>
            </div>

            <div v-if="reviewApp.ticket !== undefined"
                 class="ra-headline__intro">
               <span class="ra-ellipsis"
                     :title="reviewApp.ticket['summary']">{{ reviewApp.ticket['summary'] }}</span>
               <MDBBadge :class="{ 'jira--ready': reviewApp.ticket['status'] === 'Bereit',
                        'jira--process': reviewApp.ticket['status'] === 'In Bearbeitung',
                        'jira--review': reviewApp.ticket['status'] === 'Review',
                        'jira--done': reviewApp.ticket['status'] === 'Erledigt' }">
                  {{ reviewApp.ticket['status'] }}
               </MDBBadge>
            </div>
         </MDBCardHeader>

         <MDBCardBody>
            <div v-for="container in reviewApp.containers"
                 :key="container.name"
                 class="ra-container"
                 :class="{ 
                  'ra-container__paused': !isRunning( container ),
                  'ra-container__expandable': isExpandable( container ) 
                 }">

               <div class="ra-container__type"
                    :class="{ 'is-expanded': isExpanded( container ) }"
                    @click="toggleContainer( container )">

                  <MDBIcon
                     v-if="isExpanded( container )"
                     class="ra-icon--expander ra-icons"
                     icon="chevron-down"
                  />
                  <MDBIcon
                     v-else
                     class="ra-icon--expander ra-icons"
                     icon="chevron-right"
                  />

                  <MDBIcon v-if="container.name.endsWith( 'openid' )" class="ra-icons" icon="shield-halved" />
                  <MDBIcon v-else-if="container.name.endsWith( '-proxy' )" class="ra-icons" icon="code-branch" />
                  <MDBIcon v-else-if="container.name.endsWith( '-frontend' )" class="ra-icons" icon="globe" />
                  <MDBIcon v-else-if="container.name.endsWith( '-service' )" class="ra-icons" icon="server" />
                  <MDBIcon v-else-if="container.name.endsWith( '-api' )" class="ra-icons" icon="code" />
                  <MDBIcon
                     v-else-if="container.name.endsWith( '-db' ) || container.name.endsWith( '-database' )"
                     class="ra-icons"
                     icon="database"
                  />
                  <MDBIcon v-else class="ra-icons" icon="link" />
               </div>

               <div class="ra-container__infos">
                  <h5 class="ra-service-title">
                     <a v-if="container.url && isRunning( container )" :href='container.url' target="_blank" class="ra-service-title__name">{{ container.name }}</a>
                     <span v-else class="ra-service-title__name">{{ container.name }}</span>

                     <MDBIcon
                        class="ra-container__change-status"
                        tabindex="0"
                        v-if="reviewApp.status == 'deployed'"
                        iconStyle="far"
                        :icon="container.status === 'running' ? 'circle-pause' : 'circle-play'"
                        @click="changeState($event, container.name)"
                     />
                  </h5>

                  <div class="ra-build-infos__wrapper"
                       v-if="isExpanded( container )">
                     <div class="ra-build-infos">
                        <router-link v-if="container.openApiUrl" :to="{ name: 'open-api-ui', params: {  url: container.openApiUrl }, meta: { title: container.name }}">Open API Documentation</router-link>
                        <router-link v-if="container.asyncApiUrl" :to="{ name: 'async-api-ui', params: { url: container.asyncApiUrl }, meta: { title: container.name }}">Async API Documentation</router-link>
                        <router-link v-if="isRunning(container)" :to="{ name: 'logs', params: {  app: reviewApp.name, service: container.name }}">Logs</router-link>
                     </div>

                     <div v-if="container.version && container.version.dateModified" class="ra-build-infos">
                        <span>{{ formatBuildDate( container.version.dateModified ) }}</span>,
                        <span>{{ formatBuildTime( container.version.dateModified ) }}</span>
                     </div>
                  </div>
               </div>

               <div class="ra-container__tags">
                  <MDBBadge
                     v-mdb-tooltip="badgeTooltip( container.type )"
                     :class="badgeClass( container.type )">
                     {{ container.type }}
                  </MDBBadge>
                  <span v-if="container.version && container.version.gitCommit"
                        class="ra-build-infos ra-build-infos__hash text-end"
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
         </MDBCardBody>

         <template v-if="showOwners">
            <MDBCardFooter class="ra-owners-footer">
               <ReviewAppCardOwners :owners="reviewApp.owners" />
            </MDBCardFooter>
         </template>
      </MDBCard>

      <shutdown-app-dialog ref="deleteDlg" :app-name="reviewApp.name" v-if="reviewApp.name !== defaultAppName"/>
      <duplicate-app-dialog ref="duplicateDlg" :duplicate-from-app-name="reviewApp.name"/>
      <backup-app-dialog ref="backupDlg" :app-name="reviewApp.name" :app-status="reviewApp.status" v-if="isBackupsEnabled"/>
   </div>
</template>

<style lang="css" scoped>
.ra-owners-footer {
   padding-top: 0.5rem;
   padding-bottom: 0.5rem;
}
.ra-app-title {
   display: flex;
   align-items: center;
}
.ra-service-title {
   display: flex;
   align-items: center;
   flex-wrap: nowrap;
   gap: 0.5rem;
   font-size: 0.9rem;
   line-height: 1.2;
   min-width: 0;
}
.ra-service-title__name {
   flex: 0 1 auto;
   max-width: 100%;
   min-width: 0;
   overflow: hidden;
   text-overflow: ellipsis;
   white-space: nowrap;
}
.ra-container__change-status {
   flex-shrink: 0;
}
.ra-container-badge {
   font-size: 0.55rem;
   border: 1px solid transparent;
   font-weight: 500;
}
.ra-container-badge--instance {
   background-color: #d1d5db;
   border-color: #9ca3af;
   color: #111827;
   font-weight: 600;
}
.ra-container-badge--linked {
   background-color: #fef3c7;
   border-color: #fde68a;
   color: #92400e;
}
.ra-container-badge--replica {
   background-color: #e5e7eb;
   border-color: #d1d5db;
   color: #374151;
}
.ra-container-badge--default {
   background-color: #e2e8f0;
   border-color: #cbd5e1;
   color: #334155;
}
.badge-backed-up {
   background-color: #ef6c00;
   color: #fff;
}

.ra-menu-toggle {
   width: 2rem;
   height: 2rem;
   display: inline-flex;
   align-items: center;
   justify-content: center;
   line-height: 1;
   border-radius: 0.25rem;
}

.ra-icon--expander {
   visibility: hidden;
}

.ra-container__expandable .ra-icon--expander{
   visibility: visible;
}

.dropdown-toggle:after {
   display: none;
}
</style>

<script>
   import { ref } from 'vue';
   import moment from 'moment';
   import {
      MDBBadge,
      MDBCard,
      MDBCardBody,
      MDBCardFooter,
      MDBCardHeader,
      MDBDropdown,
      MDBDropdownItem,
      MDBDropdownMenu,
      MDBDropdownToggle,
      MDBIcon
   } from 'mdb-vue-ui-kit';
   import BackupAppDialog from './BackupAppDialog.vue';
   import DuplicateAppDialog from './DuplicateAppDialog.vue';
   import ReviewAppCardOwners from './ReviewAppCardOwners.vue';
   import ShutdownAppDialog from './ShutdownAppDialog.vue';
   import { useConfig } from '../composables/useConfig';

   export default {
      setup() {
         const { defaultAppName, isBackupsEnabled } = useConfig();
         const menuOpen = ref(false);

         return {
            defaultAppName,
            isBackupsEnabled,
            menuOpen,
         };
      },
      data() {
         return {
            expandedContainers: {}
         };
      },
      components: {
         MDBBadge,
         MDBCard,
         MDBCardBody,
         MDBCardFooter,
         MDBCardHeader,
         MDBDropdown,
         MDBDropdownItem,
         MDBDropdownMenu,
         MDBDropdownToggle,
         MDBIcon,
         'backup-app-dialog': BackupAppDialog,
         'duplicate-app-dialog': DuplicateAppDialog,
         ReviewAppCardOwners,
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
               containers.forEach((container) => {
                  this.expandedContainers[container.name] = this.isExpandable(container);
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
            const base = 'ra-container-badge';
            switch (serviceType) {
               case 'instance':
                  return `${base} ${base}--instance`;
               case 'linked':
                  return `${base} ${base}--linked`;
               case 'replica':
                  return `${base} ${base}--replica`;
            }
            return `${base} ${base}--default`;
         },
         badgeTooltip(serviceType) {
            switch (serviceType) {
               case 'instance':
                  return 'This service has been deployed especially for the review-app.';
               case 'replica':
                  return 'This service has been replicated from the service of the default review app. Changes to this service won\'t affect the service of the default review-app.';
            }
            return undefined;
         },
         toggleContainer(container) {
            if (!this.isExpandable(container)) {
               return;
            }
            this.expandedContainers[container.name] = !this.isExpanded(container);
         },
         isExpandable(container) {
            return (
               container.version != null ||
               container.openApiUrl != null ||
               container.asyncApiUrl != null ||
               this.isRunning(container)
            );
         },
         isRunning(container) {
            return container.status !== 'paused' && this.reviewApp.status !== 'backed-up';
         },
         isExpanded(container) {
            if (this.expandedContainers[container.name] == undefined) {
               return this.isExpandable(container);
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
