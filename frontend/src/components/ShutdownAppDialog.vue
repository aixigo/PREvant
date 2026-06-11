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
  <MDBModal v-model="model" size="sm" @shown="focusInput">
    <MDBModalHeader>
      <MDBModalTitle>
         Shutdown {{ appName }}
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <p>
        Do you really want to shutdown <b>{{ appName }}</b>? Confirm by typing the app name below:
      </p>
      <MDBInput
        v-model="confirmedAppName"
        ref="input"
        label="Enter app name"
        :disabled="!hasWritePermissions"
        @keyup.enter="deleteApp"
      />
      <div v-if="!hasWritePermissions" class="alert alert-warning text-center" role="alert">
         You need to be logged in to shutdown apps.
      </div>
    </MDBModalBody>

    <MDBModalFooter>
      <MDBBtn
        color="danger"
        @click="deleteApp"
        :disabled="confirmedAppName !== appName || !hasWritePermissions"
      >
        Confirm
      </MDBBtn>
    </MDBModalFooter>
  </MDBModal>
</template>

<script setup>
   import { ref, watch, defineModel, useTemplateRef } from 'vue';
   import { useStore } from 'vuex';
   import {
      MDBModal,
      MDBModalHeader,
      MDBModalTitle,
      MDBModalBody,
      MDBBtn,
      MDBModalFooter,
      MDBInput
   } from "mdb-vue-ui-kit";
   import { useAuth } from '../composables/useAuth';

   const props = defineProps({
      appName: String
   });

   const confirmedAppName = ref('');

   const model = defineModel({ default: false });
   watch(model, () => {
      confirmedAppName.value = '';
   })

   const input = useTemplateRef("input");
   function focusInput() {
      input.value?.inputRef?.focus();
   }

   const store = useStore();
   function deleteApp() {
      if (confirmedAppName.value !== props.appName) {
         return;
      }

      store.dispatch( 'deleteApp', { appName: confirmedAppName.value } );
      model.value = false
   }

   const { hasWritePermissions } = useAuth();
</script>
