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
  <MDBModal v-model="model" centered size="m" @shown="focusInput">
    <MDBModalHeader>
      <MDBModalTitle>
         Duplicate {{ duplicateFromAppName }}
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <MDBInput
         v-model.trim="newAppName"
         ref="input"
         required
         label="Enter new app name"
         :disabled="!hasWritePermissions"
         @keyup.enter="duplicateApp"
      />
      <BootstrapAlert v-if="!hasWritePermissions" type="warning" class="mt-4">
         You need to be logged in to duplicate apps.
      </BootstrapAlert>
    </MDBModalBody>

    <MDBModalFooter>
      <MDBBtn color="primary" :disabled="!newAppName || !hasWritePermissions" @click="duplicateApp">
        Duplicate
      </MDBBtn>
    </MDBModalFooter>
  </MDBModal>
</template>


<script setup>
   import { ref, watch, useTemplateRef } from 'vue';
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
   import BootstrapAlert from './bootstrap/BootstrapAlert.vue';

   const props = defineProps({
      duplicateFromAppName: String
   });

   const newAppName = ref('');

   const model = defineModel({ default: false });
   watch(model, () => {
      newAppName.value = '';
   })

   const input = useTemplateRef("input");
   function focusInput() {
      input.value?.inputRef?.focus();
   }

   const store = useStore();
   function duplicateApp() {
      store.dispatch( 'duplicateApp', {
         appToDuplicate: props.duplicateFromAppName,
         newAppName: newAppName.value
      } );
      model.value = false;
   }

   const { hasWritePermissions } = useAuth();
</script>
