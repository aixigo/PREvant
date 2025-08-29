<template>
  <Dialog
    ref="dialog"
    title="API Documentation"
    large
    @close="handleClose"
  >
    <template v-slot:body>
      <div class="open-api-ui" ref="openapi"></div>
    </template>
  </Dialog>
</template>

<style lang="css" src="swagger-ui/dist/swagger-ui.css"></style>

<style lang="scss" scoped>
/* Fixes swagger ui response column width */
.open-api-ui {
  :deep(.response-col_status) {
    width: 10% !important;
  }
  :deep(.parameters-col_name) {
    width: 10% !important;
  }
}
</style>

<script setup>
import { onMounted, useTemplateRef } from "vue";
import { useRoute, useRouter } from "vue-router";
import SwaggerUI from "swagger-ui";
import Dialog from "../components/Dialog.vue";

const route = useRoute();
const router = useRouter();

const dialog = useTemplateRef("dialog");
const openapi = useTemplateRef("openapi");

onMounted(() => {
  dialog.value.open();

  SwaggerUI({
    url: route.params.url,
    domNode: openapi.value,
  });
});

function handleClose() {
  router.push(router.options.history.state.back ?? "/");
}
</script>