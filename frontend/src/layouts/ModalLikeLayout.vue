<template>
  <transition name="modal">
    <div class="ra-modal-mask">
      <div class="ra-modal-wrapper">
        <div class="ra-modal-container">
          <div class="ra-modal-header">
            <h1>
              {{ title }}
              <template v-if="titleSuffix"> – {{ titleSuffix }}</template>
            </h1>
            <span @click="close">
              <font-awesome-icon
                icon="window-close"
                class="ra-modal-close-button"
                aria-label="Close"
              />
            </span>
          </div>

          <div class="ra-modal-body">
            <slot />
          </div>
        </div>
      </div>
    </div>
  </transition>
</template>

<script setup>
import { useRouter } from "vue-router";
import { useEscape } from "../composables/useEscape";

const props = defineProps({
  title: String,
  titleSuffix: String
});

const router = useRouter();
function close() {
  router.push(router.options.history.state.back ?? "/");
}
useEscape(close);
</script>

<style type="text/css" scoped>
   .ra-modal-close-button {
       position: absolute;
       right: 20px;
       top: 20px;
   }

   .ra-modal-mask {
      position: fixed;
      z-index: 9998;
      top: 0;
      left: 0;
      width: 100%;
      height: 100%;
      background-color: rgba(0, 0, 0, .5);
      display: table;
      transition: opacity .3s ease;
   }

   .ra-modal-wrapper {
      display: table-cell;
      vertical-align: middle;
   }

   .ra-modal-container {
      position: fixed;
      top: 50px;
      left: 50px;
      bottom: 50px;
      right: 50px;
      padding: 20px 30px;
      background-color: #fff;
      border-radius: 2px;
      box-shadow: 0 2px 8px rgba(0, 0, 0, .33);
      transition: all .3s ease;
      font-family: Helvetica, Arial, sans-serif;

      overflow-y: scroll;
   }

   .ra-modal-body {
      margin: 20px 0;
   }

   .ra-modal-enter .ra-modal-container,
   .ra-modal-leave-active .ra-modal-container {
      -webkit-transform: scale(0.1);
      transform: scale(0.1);
   }
</style>