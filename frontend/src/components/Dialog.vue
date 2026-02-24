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
   <div ref="dialog" class="modal fade" tabindex="-1" role="dialog">
      <div class="modal-dialog modal-dialog-centered" :class="{ 'modal-large': large }" role="document">
         <div class="modal-content">
            <div class="modal-header">
               <h5 class="modal-title">{{ title }}</h5>
               <button type="button" class="close" data-dismiss="modal" aria-label="Close">
                  <span aria-hidden="true">&times;</span>
               </button>
            </div>
            <div class="modal-body">
               <slot name="body"></slot>

               <div class="alert alert-danger" role="alert" v-if="errorStatusText">
                  {{errorStatusText}} <span v-if="errorStatus" class="badge badge-danger">{{errorStatus}}</span>
               </div>
            </div>
            <div class="modal-footer">
               <slot name="footer"></slot>
            </div>
         </div>
      </div>
   </div>
</template>

<style>
   .modal-large {
      max-width: 90vw !important;
   }
</style>

<script>
   export default {
      data() {
         return {}
      },
      props: {
         title: {type: String},
         errorStatusText: {type: String},
         errorStatus: {type: Number},
         large: {type: Boolean, default: false}
      },
      mounted() {
         $(this.$refs.dialog).on('shown.bs.modal', () => {
            this.$emit('opened');
         })
         $(this.$refs.dialog).on('hide.bs.modal', () => {
            this.$emit('close');
         })
      },
      methods: {
         open() {
            // see configuration options: https://getbootstrap.com/docs/4.0/components/modal/#options
            $(this.$refs.dialog).modal({
               backdrop: true,
               keyboard: true,
               focus: true
            });
         },

         close() {
            $(this.$refs.dialog).modal('hide');
         }
      },
   }
</script>
