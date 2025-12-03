/**
 * Injects overrides into any global object (e.g. window.config)
 * and ignores anything set by the vue app.
 */
export async function injectGlobalOverride(page, key, value) {
  await page.addInitScript(
    ({ globalKey, globalValue }) => {
      Object.defineProperty(window, globalKey, {
        configurable: true,
        enumerable: true,
        get() {
          return globalValue;
        },
        set() {
          // ignore any writes done by the app
        },
      });
    },
    { globalKey: key, globalValue: value }
  );
}
