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
   <dlg ref="dialog" :title="`Logs of ${serviceName} in ${appName}`" :large="true" @close="clearLogs">
      <template slot="body">
         <recycle-scroller ref="recycleScroller" class="ra-logs" :item-size="20" :items="logLines"
                           emit-update @update="updateLogs">
            <template v-slot="{ item }">
               <p class="ra-log-line">{{ item.line }}</p>
            </template>
         </recycle-scroller>
      </template>
   </dlg>
</template>

<style>
   .ra-logs {
      height: 80vh;
      overflow: auto;

      background-color: black;
      color: white;
      font-family: var(--font-family-monospace);

      padding: 0.5rem;
   }

   .ra-log-line {
      padding: 0;
      white-space: nowrap;
      height: 20px;
      line-height: 20px;
      margin: 0;
   }

   /*
    This is a workaround for vue-recycle-scroller because it sets the css property transform with translateY(...px)
    for each line but without making the positioning of the lines relative (see
    https://github.com/Akryum/vue-virtual-scroller/issues/196 for more information).
   */
   .ra-logs .vue-recycle-scroller__item-wrapper {
      position: relative;
   }

   .ra-logs .vue-recycle-scroller__item-view {
      position: absolute;
   }
</style>

<script>
   import Dialog from './Dialog.vue';
   import { RecycleScroller } from 'vue-virtual-scroller';
   import LinkHeader from 'http-link-header';

   let requestUri;

   export default {
      data() {
         return {
            logLines: [],
            currentPageLink: null,
            nextPageLink: null,
            logsFrom: {
               appName: this.appName,
               serviceName: this.serviceName
            }
         };
      },
      components: {
         'dlg': Dialog,
         'recycle-scroller': RecycleScroller
      },
      props: {
         appName: { type: String },
         serviceName: { type: String }
      },
      watch: {
         appName( newAppName ) {
            this.logsFrom.appName = newAppName;

            if ( newAppName != null && this.logsFrom.serviceName != null ) {
               this.currentPageLink = `/api/apps/${this.logsFrom.appName}/logs/${this.logsFrom.serviceName}`;
            }
         },
         serviceName( newServiceName ) {
            this.logsFrom.serviceName = newServiceName;

            if ( this.logsFrom.appName != null && newServiceName != null ) {
               this.currentPageLink = `/api/apps/${this.logsFrom.appName}/logs/${this.logsFrom.serviceName}`;
            }
         },
         currentPageLink( newCurrentPageLink ) {
            this.fetchLogs( newCurrentPageLink );
         }
      },
      methods: {
         open() {
            this.$refs.dialog.open();
         },

         fetchLogs( newRequestUri ) {
            if ( newRequestUri == null || requestUri != null ) {
               return;
            }

            requestUri = newRequestUri;

            fetch( requestUri )
               .then( parseLogsResponse )
               .then( ( { logLines, rel } ) => {
                     requestUri = null;
                     this.nextPageLink = rel.uri;

                     const linesSplit = logLines.split( '\n' );
                     this.logLines = this.logLines.concat(
                        linesSplit
                           .filter( ( line, index ) => index < linesSplit.length - 1 )
                           .map( ( line, index ) => ( { id: index, line } ) )
                     );
                  }
               )
               .catch( () => {
                  requestUri = null;
               } )
         },

         clearLogs() {
            this.currentPageLink = null;
            this.nextPageLink = null;
            this.logLines = [];
         },

         updateLogs( startIndex, endIndex ) {
            if ( endIndex >= this.logLines.length && this.nextPageLink ) {
               const nextPageLink = this.nextPageLink;
               this.nextPageLink = null;
               this.currentPageLink = nextPageLink;
            }
         }
      }
   }

   function parseLogsResponse( response ) {
      return new Promise( ( resolve, reject ) => {
         if ( !response.ok ) {
            return reject( response );
         }

         const link = response.headers.get( 'Link' );
         let rel = null;
         if ( link != null ) {
            const linkHeader = LinkHeader.parse( link );
            rel = linkHeader.get( 'rel', 'next' ).find( link => link.uri != null );
         }
         return resolve( response.text().then( text => ( { logLines: text, rel } ) ) );
      } );
   }
</script>