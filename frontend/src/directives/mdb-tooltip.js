import { createApp, defineComponent, h, reactive } from "vue";
import { MDBTooltip } from "mdb-vue-ui-kit";

/**
 * Global `v-mdb-tooltip` directive.
 *
 * The MDB component library does not offer a tooltip directive out of the box.
 * In order to avoid overly verbose markup everytime we want to add a simple
 * tooltip we introduce this custom directive which can be used on any DOM
 * element to add a tooltip text.
 *
 * Usage:
 * - `v-mdb-tooltip="'Some more text'"`
 */

const DEFAULT_MAX_WIDTH = 320;
const DEFAULT_DIRECTION = "top";
const TOOLTIP_TARGET_ATTR = "data-mdb-tooltip-target";
let tooltipId = 0;

/**
 * Mounts tooltip host app and attaches interaction listeners to the target element.
 *
 * Why create a local Vue app here?
 * Vue directives can only operate on raw DOM nodes and cannot directly render
 * Vue components. Because `MDBTooltip` is a Vue component, we create a tiny
 * local app (`TooltipHost`) and mount it into an internal host element in
 * `document.body`. The directive target element is then used as the tooltip
 * reference/anchor.
 *
 * @param {HTMLElement & { __mdbTooltip?: any }} el
 * @param {{ value: unknown }} binding
 */
function mountTooltip(el, binding) {
  const id = `mdb-tooltip-target-${(tooltipId += 1)}`;
  const selector = `[${TOOLTIP_TARGET_ATTR}="${id}"]`;
  const state = reactive({
    text: binding.value ?? "",
    visible: false,
  });

  el.setAttribute(TOOLTIP_TARGET_ATTR, id);

  const host = document.createElement("span");
  host.className = "mdb-tooltip-directive-host";
  document.body.appendChild(host);

  const TooltipHost = defineComponent({
    name: "MdbTooltipDirectiveHost",
    setup() {
      return () =>
        h(
          MDBTooltip,
          {
            modelValue: state.visible,
            "onUpdate:modelValue": (nextValue) => {
              state.visible = nextValue;
            },
            reference: selector,
            direction: DEFAULT_DIRECTION,
            maxWidth: DEFAULT_MAX_WIDTH,
            arrow: true,
            disabled: state.text.length === 0,
          },
          {
            reference: () => h("span", { "aria-hidden": "true" }),
            tip: () => state.text,
          },
        );
    },
  });

  const app = createApp(TooltipHost);
  app.mount(host);

  const show = () => {
    if (state.text.length === 0) {
      return;
    }
    state.visible = true;
  };
  const hide = () => {
    state.visible = false;
  };

  el.addEventListener("mouseenter", show);
  el.addEventListener("mouseleave", hide);
  el.addEventListener("focusin", show);
  el.addEventListener("focusout", hide);

  el.__mdbTooltip = {
    app,
    host,
    state,
    show,
    hide,
  };
}

/**
 * Removes listeners and destroys the mounted tooltip host app.
 *
 * @param {HTMLElement & { __mdbTooltip?: any }} el
 */
function unmountTooltip(el) {
  const tooltipContext = el.__mdbTooltip;
  if (!tooltipContext) {
    return;
  }

  el.removeEventListener("mouseenter", tooltipContext.show);
  el.removeEventListener("mouseleave", tooltipContext.hide);
  el.removeEventListener("focusin", tooltipContext.show);
  el.removeEventListener("focusout", tooltipContext.hide);
  el.removeAttribute(TOOLTIP_TARGET_ATTR);

  tooltipContext.app.unmount();
  tooltipContext.host.remove();

  delete el.__mdbTooltip;
}

/**
 * Vue directive implementation for `v-mdb-tooltip`.
 */
const mdbTooltipDirective = {
  mounted(el, binding) {
    mountTooltip(el, binding);
  },
  updated(el, binding) {
    const tooltipContext = el.__mdbTooltip;
    if (!tooltipContext) {
      mountTooltip(el, binding);
      return;
    }

    tooltipContext.state.text = binding.value ?? "";

    if (tooltipContext.state.text.length === 0) {
      tooltipContext.state.visible = false;
    }
  },
  unmounted(el) {
    unmountTooltip(el);
  },
};

export default mdbTooltipDirective;
