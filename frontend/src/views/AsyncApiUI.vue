<template>
   <transition name="modal">
      <div class="ra-modal-mask">
         <div class="ra-modal-wrapper">
            <div class="ra-modal-container">

               <div class="ra-modal-header">
                  <h1>AsyncAPI Documentation<span v-if="showAdditionalHeadlineInformation"> – {{ $route.params.title }}</span></h1>

                  <span @click="close()">
                     <font-awesome-icon icon="window-close" class="ra-modal-close-button" aria-label="Close"/>
                  </span>
               </div>

               <div class="ra-modal-body">
                  <div ref="asyncapi"></div>
               </div>
            </div>
         </div>
      </div>
   </transition>
</template>

<style scope src="@asyncapi/react-component/styles/default.min.css"></style>

<script setup>
import AsyncApiStandalone from "@asyncapi/react-component/browser/standalone";
import { computed, onMounted, useTemplateRef } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useEscape } from "../composables/useEscape";

const asyncapi = useTemplateRef("asyncapi");
onMounted(() => {
  AsyncApiStandalone.render(
    {
      schema: { url: route.params.url },
      config: {},
    },
    asyncapi.value
  );
});

const route = useRoute();
const showAdditionalHeadlineInformation = computed(
  () => route.params.title != null
);

const router = useRouter();
function close() {
  router.push(router.options.history.state.back ?? "/");
}
useEscape(close);
</script>
