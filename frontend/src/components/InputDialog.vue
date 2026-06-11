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
   <MDBModal v-model="visible" centered @shown="focusInput">
      <MDBModalHeader>
         <MDBModalTitle>
            {{ title }}
         </MDBModalTitle>
      </MDBModalHeader>

      <MDBModalBody>
         <slot name="description">
            <p v-if="description">{{ description }}</p>
         </slot>

         <MDBInput
            ref="inputElement"
            v-model="inputValue"
            :label="inputPlaceholder"
            :disabled="!isActionAllowed"
            @keyup.enter="confirm" />

         <BootstrapAlert v-if="requiresWritePermissions && !hasWritePermissions && authMessage != null" type="warning" class="mt-4">
            {{ authMessage }}
         </BootstrapAlert>
      </MDBModalBody>

      <MDBModalFooter>
         <button
            type="button"
            :class="buttonClass"
            @click="confirm()"
            :disabled="!canConfirm">
            {{ confirmLabel }}
         </button>
      </MDBModalFooter>
   </MDBModal>
</template>

<script setup>
   import { computed, ref, useTemplateRef } from 'vue';
   import {
      MDBModal,
      MDBModalHeader,
      MDBModalTitle,
      MDBModalBody,
      MDBModalFooter,
      MDBInput
   } from 'mdb-vue-ui-kit';
   import { useAuth } from '../composables/useAuth';
   import BootstrapAlert from './bootstrap/BootstrapAlert.vue';

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

   const inputElement = useTemplateRef('inputElement');
   const visible = ref(false);
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
      visible.value = true;
   }

   function focusInput() {
      inputElement.value?.inputRef?.focus();
   }

   function confirm() {
      if (!canConfirm.value) {
         return;
      }

      emit('confirm', normalizedInput.value);
      visible.value = false;
   }

   defineExpose({
      open
   });
</script>
