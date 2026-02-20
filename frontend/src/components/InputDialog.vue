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
   <Dialog ref="dialog" :title="title" @opened="focusInput">
      <template #body>
         <slot name="description">
            <p v-if="description">{{ description }}</p>
         </slot>

         <div class="form-group">
            <input
               ref="inputElement"
               type="name"
               class="form-control"
               :placeholder="inputPlaceholder"
               v-model="inputValue"
               :disabled="!isActionAllowed"
               @keyup.enter="confirm">
         </div>

         <div v-if="requiresWritePermissions && !hasWritePermissions && authMessage != null" class="alert alert-warning text-center" role="alert">
            {{ authMessage }}
         </div>
      </template>

      <template #footer>
         <button
            type="button"
            :class="buttonClass"
            @click="confirm()"
            :disabled="!canConfirm">
            {{ confirmLabel }}
         </button>
      </template>
   </Dialog>
</template>

<script setup>
   import { computed, ref, useTemplateRef } from 'vue';
   import { useAuth } from '../composables/useAuth';
   import Dialog from './Dialog.vue';

   const { hasWritePermissions } = useAuth();

   const props = defineProps({
      title: { type: String, required: true },
      description: { type: String, default: '' },
      expectedValue: { type: String, default: '' },
      requireMatch: { type: Boolean, default: false },
      trimInput: { type: Boolean, default: false },
      requiresWritePermissions: { type: Boolean, default: true },
      confirmLabel: { type: String, required: true },
      authMessage: { type: String, default: undefined },
      inputPlaceholder: { type: String, default: 'Enter app name' },
      buttonClass: { type: String, default: 'btn btn-outline-primary' },
   });

   const emit = defineEmits(['confirm']);

   const dialog = useTemplateRef('dialog');
   const inputElement = useTemplateRef('inputElement');
   const inputValue = ref('');

   const normalizedInput = computed(() => {
      return props.trimInput ? inputValue.value.trim() : inputValue.value;
   });

   const isActionAllowed = computed(() => {
      return !props.requiresWritePermissions || hasWritePermissions.value;
   });

   const canConfirm = computed(() => {
      if (!isActionAllowed.value) {
         return false;
      }
      if (props.requireMatch) {
         return inputValue.value === props.expectedValue;
      }
      return normalizedInput.value.length > 0;
   });

   function open() {
      inputValue.value = '';
      dialog.value.open();
   }

   function focusInput() {
      if (inputElement.value && !inputElement.value.disabled) {
         inputElement.value.focus();
      }
   }

   function confirm() {
      if (!canConfirm.value) {
         return;
      }

      emit('confirm', normalizedInput.value);
      dialog.value.close();
   }

   defineExpose({
      open
   });
</script>
