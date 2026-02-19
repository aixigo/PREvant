/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 - 2026 aixigo AG
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
   <ConfirmationDialog
      ref="dialog"
      :title="'Shutdown ' + appName"
      :expected-value="appName"
      confirm-label="Confirm"
      auth-message="You need to be logged in to shutdown apps."
      button-class="btn btn-outline-danger"
      @confirm="deleteApp">
      <template #description>
         <p>Do you really want to shutdown <b>{{ appName }}</b>? Confirm by typing in the app:</p>
      </template>
   </ConfirmationDialog>
</template>

<script setup>
   import { useTemplateRef } from 'vue';
   import { useStore } from 'vuex';
   import ConfirmationDialog from './ConfirmationDialog.vue';

   defineProps({
      appName: { type: String, required: true }
   });

   const store = useStore();
   const dialog = useTemplateRef('dialog');

   function open() {
      dialog.value.open();
   }

   function deleteApp(appName) {
      store.dispatch('deleteApp', { appName });
   }

   defineExpose({
      open
   });
</script>
