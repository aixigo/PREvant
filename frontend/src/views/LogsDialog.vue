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
  <MDBModal
    v-model="showModal"
    centered
    size="xl"
    @hidden="clearLogs"
    @hide="destroyScrollContainer"
    @show="initScrollContainer"
  >
    <MDBModalHeader>
      <MDBModalTitle>
        Logs of {{ $route.params.service }} in {{ $route.params.app }}
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <div class="d-flex justify-content-between align-items-center gap-4 mb-2">
         <BootstrapAlert type="primary" class="m-0 me-auto" :class="{ invisible: scrollPosition !== 0}">
            Maximum Log View Reached. For complete log details, please use the
            'Download Full Logs' button.
         </BootstrapAlert>

        <MDBBtn
          outline="primary"
          class="ml-auto"
          tag="a" 
          :href="downloadLink"
          download="filename.txt"
        >
          <font-awesome-icon icon="download" />
          &nbsp;Download Full Logs
        </MDBBtn>
      </div>

      <DynamicScroller
        ref="scroller"
        :items="logLines"
        :min-item-size="20"
        :item-size="itemSize"
        class="ra-logs"
        :buffer="500"
      >
        <template v-slot="{ item, index, active }">
          <DynamicScrollerItem
            :item="item"
            :active="active"
            :size-dependencies="[item.line]"
            :data-index="index"
            :data-active="active"
          >
            <div class="ra-log-line" :key="item.id">{{ item.line }}</div>
          </DynamicScrollerItem>
        </template>
      </DynamicScroller>
    </MDBModalBody>

    <MDBModalFooter>
      <MDBBtn color="secondary" @click="showModal = false">Close</MDBBtn>
    </MDBModalFooter>
  </MDBModal>
</template>

<style>
@import 'vue-virtual-scroller/dist/vue-virtual-scroller.css';

.ra-logs {
   height: 70vh;
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
   import { ref } from "vue";
   import {
      MDBModal,
      MDBModalHeader,
      MDBModalTitle,
      MDBModalBody,
      MDBBtn,
      MDBModalFooter,
   } from "mdb-vue-ui-kit";
   import LinkHeader from 'http-link-header';
   import { DynamicScroller, DynamicScrollerItem } from 'vue-virtual-scroller';
   import moment from 'moment';
   import { useCloseNavigation } from '../composables/useCloseNavigation';
   import BootstrapAlert from "../components/bootstrap/BootstrapAlert.vue";

   let requestUri;

   export default {
      setup() {
         const showModal = ref(false);
         const { navigateOnClose } = useCloseNavigation();

         return {
            showModal,
            navigateOnClose
         };
      },
      data() {
         return {
            itemSize: 24,
            logLines: [],
            eventSource: null,
            scrollPosition: null,
            nextPageLink: null,
            errorText: '',
         };
      },
      components: {
         BootstrapAlert,
         MDBModal,
         MDBModalHeader,
         MDBModalTitle,
         MDBModalBody,
         MDBBtn,
         MDBModalFooter,
         DynamicScrollerItem: DynamicScrollerItem,
         DynamicScroller: DynamicScroller,
      },
      computed: {
         currentPageLink() {
            const since = moment().subtract(24, 'hours').toISOString();
            return `/api/apps/${this.$route.params.app}/logs/${this.$route.params.service}?since=${since}`;
         },
         downloadLink() {
            return `/api/apps/${this.$route.params.app}/logs/${this.$route.params.service}?asAttachment=true`;
         },
      },
      mounted() {
         this.fetchLogs(this.currentPageLink);
         this.showModal = true;
      },
      beforeDestroy() {
         this.errorText = '';
         this.logLines = [];
         if (this.eventSource) {
            this.eventSource.close();
         }
      },
      methods: {
         initScrollContainer() {
            this.$refs.scroller?.$el.addEventListener('scroll', this.handleScroll);
         },
         destroyScrollContainer() {
            this.$refs.scroller?.$el.removeEventListener('scroll', this.handleScroll);
         },
         fetchLogs(newRequestUri) {
            if (newRequestUri == null || requestUri != null) {
               return;
            }
            this.logLines = [];
            requestUri = newRequestUri;

            fetch(requestUri)
               .then(parseLogsResponse)
               .then(({ logLines, rel }) => {
                  requestUri = null;
                  this.nextPageLink = rel;
                  const linesSplit = logLines.split('\n');
                  this.logLines = this.logLines.concat(
                     linesSplit
                        .filter((line, index) => index < linesSplit.length - 1)
                        .map((line, index) => ({ id: index, line }))
                  );
               })
               .then(() => {
                  this.fetchLogStream();
               })
               .catch(() => {
                  requestUri = null;
               });
         },

         fetchLogStream() {
            this.eventSource = new EventSource(this.nextPageLink);
            this.eventSource.onopen = () => {
               this.$nextTick(() => {
                  this.scrollBottom();
               });
            };

            this.eventSource.addEventListener('message', (e) => {
               const nextId = this.logLines.length > 0 ? this.logLines[this.logLines.length - 1].id + 1 : 1;
               this.logLines.push({ id: nextId, line: e.data });
               if (this.isCloseToBottom()) {
                  this.$nextTick(() => {
                     this.scrollBottom();
                  });
               }
            });
         },

         clearLogs() {
            this.nextPageLink = null;
            this.logLines = [];
            if (this.eventSource) {
               this.eventSource.close();
            }
            this.navigateOnClose();
         },

         scrollBottom() {
            const scroller = this.$refs.scroller;
            scroller.scrollToBottom();
         },

         isCloseToBottom() {
            const el = this.$refs.scroller.$el;
            const distanceFromBottom =
               el.scrollHeight - (el.scrollTop + el.clientHeight);
            return distanceFromBottom < this.itemSize;
         },

         handleScroll() {
            this.scrollPosition = this.$refs.scroller.$el.scrollTop;
         },
      },
   };

   function parseLogsResponse(response) {
      return new Promise((resolve, reject) => {
         if (!response.ok) {
            return reject(response);
         }
         const link = response.headers.get('Link');
         let rel = null;
         if (link != null) {
            const ref = LinkHeader.parse(link).refs.find(ref => {
               return ref.rel === "next";
            });

            if (ref != null) {
               rel = ref.uri;
            }
         }

         return resolve(response.text().then((text) => ({ logLines: text, rel })));
      });
   }
</script>
