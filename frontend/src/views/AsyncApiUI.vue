<template>
  <Dialog
    ref="dialog"
    title="AsyncAPI Documentation"
    large
    @close="handleClose"
  >
    <template v-slot:body>
      <div ref="asyncapi"></div>
    </template>
  </Dialog>
</template>

<style src="@asyncapi/react-component/styles/default.min.css"></style>

<script setup>
import { onMounted, useTemplateRef } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncApiStandalone from "@asyncapi/react-component/browser/standalone";
import Dialog from "../components/Dialog.vue";

const route = useRoute();
const router = useRouter();

const dialog = useTemplateRef("dialog");
const asyncapi = useTemplateRef("asyncapi");

onMounted(() => {
  dialog.value.open();

  AsyncApiStandalone.render(
    {
      schema: { url: route.params.url },
      config: {},
    },
    asyncapi.value
  );
});

function handleClose() {
  router.push(router.options.history.state.back ?? "/");
}
</script>