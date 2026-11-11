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
    @hide="handleClose"
  >
    <MDBModalHeader>
      <MDBModalTitle>
        Logs of {{ $route.params.service }} in {{ $route.params.app }}
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <div class="d-flex justify-content-between align-items-center gap-4 mb-2">
         <BootstrapAlert type="primary" class="m-0 me-auto" :class="{ invisible: !showReachedTopMessage }">
            Maximum Log View Reached. <br/>
            For complete log details, please use the 'Download Full Logs' button.
         </BootstrapAlert>

        <MDBBtn
          outline="primary"
          class="ms-auto"
          tag="a"
          :href="downloadLink"
          download="filename.txt"
        >
          <MDBIcon icon="download" />
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
        :emit-update="true"
         @scroll-start="showReachedTopMessage = true;"
         @scroll-end="showReachedTopMessage = false;"
      >
        <template v-slot="{ item, index, active }">
          <DynamicScrollerItem
            :item="item"
            :index="index"
            :active="active"
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

<script setup>
import { EventSource } from 'eventsource';
import { ref, onMounted, watch, nextTick } from "vue";
import { useRoute } from "vue-router";
import {
   MDBModal,
   MDBModalHeader,
   MDBModalTitle,
   MDBModalBody,
   MDBBtn,
   MDBModalFooter,
   MDBIcon,
} from "mdb-vue-ui-kit";
import { DynamicScroller, DynamicScrollerItem } from 'vue-virtual-scroller';
import { useCloseNavigation } from '../composables/useCloseNavigation';
import BootstrapAlert from "../components/bootstrap/BootstrapAlert.vue";

const scroller = ref(null);
const showModal = ref(false);
const showReachedTopMessage = ref(false);
const { navigateOnClose } = useCloseNavigation();

const logLines = ref([]);
const itemSize = ref(24);
const eventSource = ref(null);

const route = useRoute();
const downloadLink = `/api/apps/${route.params.app}/logs/${route.params.service}?asAttachment=true`;

onMounted(() => {
  showModal.value = true;
});

watch(() => route.params, subscribeToLogs, { immediate: true });
watch(() => logLines.value.length, async () => {
   const wasCloseToBottom = isCloseToBottom();
   await nextTick();
   if (wasCloseToBottom) {
      requestAnimationFrame(() => scrollToBottom());
   }
})

function isCloseToBottom() {
    const el = scroller.value?.$el;
    if (!el) return true;  // fallback: scroll if we can't determine
    const distance = el.scrollHeight - (el.scrollTop + el.clientHeight);
    return distance < itemSize.value;
}

function scrollToBottom() {
    scroller.value?.scrollToBottom();
}

function handleClose() {
   logLines.value = [];
   if (eventSource.value != null) {
      eventSource.value.close();
   }
   navigateOnClose();
}

async function subscribeToLogs(params) {
   const since = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString()
   const logsUrl = `/api/apps/${params.app}/logs/${params.service}?since=${since}`;

   eventSource.value = new EventSource(logsUrl);
   eventSource.value.addEventListener('message', (e) => {
      const length = logLines.value.length;
      const nextId = length > 0 ? logLines.value[length - 1].id + 1 : 1;
      logLines.value.push({ id: nextId, line: e.data });
   });
}
</script>
