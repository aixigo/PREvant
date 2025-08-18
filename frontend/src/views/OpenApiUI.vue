<template>
  <ModalLikeLayout title="API Documentation" :title-suffix="route.query.title">
    <div class="open-api-ui" ref="asyncapi"></div>
  </ModalLikeLayout>
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
import { useRoute } from "vue-router";
import SwaggerUI from "swagger-ui";
import ModalLikeLayout from "../layouts/ModalLikeLayout.vue";

const route = useRoute();
const asyncapi = useTemplateRef("asyncapi");
onMounted(() => {
  SwaggerUI({
    url: route.params.url,
    domNode: asyncapi.value,
  });
});
</script>
