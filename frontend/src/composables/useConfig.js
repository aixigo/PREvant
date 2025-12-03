import { computed } from "vue";

export function useConfig() {
  const injectedConfig = computed(() => window.config ?? {});
  const defaultAppName = computed(() => injectedConfig.value.defaultAppName);
  const isAuthRequired = computed(() => injectedConfig.value.isAuthRequired ?? false);

  return {
    defaultAppName,
    isAuthRequired,
  };
}
