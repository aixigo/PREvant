<template>
  <ModalLikeLayout
    title="AsyncAPI Documentation "
    :title-suffix="route.query.title"
  >
    <div ref="asyncapi"></div>
  </ModalLikeLayout>
</template>

<style src="@asyncapi/react-component/styles/default.min.css"></style>

<script setup>
import { onMounted, useTemplateRef } from "vue";
import { useRoute } from "vue-router";
import AsyncApiStandalone from "@asyncapi/react-component/browser/standalone";
import ModalLikeLayout from "../layouts/ModalLikeLayout.vue";

const route = useRoute();
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
</script>
