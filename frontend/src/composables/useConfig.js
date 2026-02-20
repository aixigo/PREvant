import { computed } from "vue";

export function useConfig() {
  const injectedConfig = computed(() => window.config ?? {});
  const defaultAppName = computed(() => injectedConfig.value.defaultAppName);
  const isAuthRequired = computed(() => injectedConfig.value.isAuthRequired ?? false);
  const isBackupsEnabled = computed(
    () => injectedConfig.value.isBackupsEnabled ?? false
  );

  return {
    defaultAppName,
    isAuthRequired,
    isBackupsEnabled,
  };
}
