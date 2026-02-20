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
   <InputDialog
      ref="dialog"
      :title="title"
      :description="description"
      :expected-value="expectedValue"
      :require-match="true"
      :trim-input="trimInput"
      :requires-write-permissions="requiresWritePermissions"
      :confirm-label="confirmLabel"
      :auth-message="authMessage"
      :input-placeholder="inputPlaceholder"
      :button-class="buttonClass"
      @confirm="forwardConfirm">
      <template v-if="$slots.description" #description>
         <slot name="description"></slot>
      </template>
   </InputDialog>
</template>

<script setup>
   import { useTemplateRef } from 'vue';
   import InputDialog from './InputDialog.vue';

   defineProps({
      title: { type: String, required: true },
      description: { type: String, default: '' },
      expectedValue: { type: String, required: true },
      trimInput: { type: Boolean, default: false },
      requiresWritePermissions: { type: Boolean, default: true },
      confirmLabel: { type: String, required: true },
      authMessage: { type: String, default: undefined },
      inputPlaceholder: { type: String, default: 'Enter app name' },
      buttonClass: { type: String, default: 'btn btn-outline-primary' },
   });

   const emit = defineEmits(['confirm']);
   const dialog = useTemplateRef('dialog');

   function open() {
      dialog.value.open();
   }

   function forwardConfirm(value) {
      emit('confirm', value);
   }

   defineExpose({
      open
   });
</script>
