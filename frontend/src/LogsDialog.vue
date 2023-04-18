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
   <dlg ref="dialog" :title="`Logs of ${$route.params.service} in ${$route.params.app}`" :large="true" @close="clearLogs">
      <template v-slot:body>
         <DynamicScroller :items="logLines" :item-size="20" class="ra-logs">
            <template v-slot="{ item, index, active }">
               <DynamicScrollerItem :item="item" :active="active" :size-dependencies="[ item.line ]" :data-index="index">
                  <div class="ra-log-line" :key="item.id">
                     {{ item.line }}
                  </div>
               </DynamicScrollerItem>
            </template>
         </DynamicScroller>
      </template>
   </dlg>
</template>

<style>
   @import 'vue-virtual-scroller/dist/vue-virtual-scroller.css';

   .ra-logs {
      height: 80vh;
      overflow: auto;

      background-color: black;
      color: white;
      font-family: var(--font-family-monospace);

      padding: 0.5rem;
   }

   .ra-log-line {
      white-space: nowrap;
      height: 20px;
   }
</style>

<script>
   import Dialog from './Dialog.vue';
   import parseLinkHeader from 'parse-link-header';
   import { DynamicScroller, DynamicScrollerItem } from 'vue-virtual-scroller'

   let requestUri;

   export default {
      data() {
         return {
            logLines: [],
            nextPageLink: null
         };
      },
      components: {
         'dlg': Dialog,
         'DynamicScrollerItem': DynamicScrollerItem,
         'DynamicScroller': DynamicScroller
      },
      watch: {
         currentPageLink(newCurrentPageLink) {
            this.logLines = [];
            this.fetchLogs( newCurrentPageLink );
         }
      },
      computed: {
         currentPageLink() {
            return `/api/apps/${this.$route.params.app}/logs/${this.$route.params.service}`;
         }
      },
      mounted() {
         this.fetchLogs( this.currentPageLink );
         this.$refs.dialog.open();
      },
      methods: {
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

            this.$router.push('/');
         },

         updateLogs() {
            if ( this.nextPageLink ) {
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
            const linkHeader = parseLinkHeader( link );
            if ( linkHeader.next != null ) {
               rel = linkHeader.next.url;
            }
         }
         return resolve( response.text().then( text => ( { logLines: text, rel } ) ) );
      } );
   }
</script>
