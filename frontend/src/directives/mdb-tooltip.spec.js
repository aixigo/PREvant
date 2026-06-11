/* @vitest-environment jsdom */
import { defineComponent, nextTick, h } from "vue";
import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import mdbTooltipDirective from "./mdb-tooltip";

vi.mock("mdb-vue-ui-kit", async () => {
  const MDBTooltip = defineComponent({
    name: "MDBTooltip",
    props: {
      modelValue: Boolean,
      direction: String,
      maxWidth: Number,
      arrow: Boolean,
      disabled: Boolean,
    },
    setup(props, { slots }) {
      return () =>
        h(
          "div",
          {
            class: "mock-mdb-tooltip",
            "data-visible": String(props.modelValue),
            "data-direction": props.direction,
            "data-max-width": String(props.maxWidth),
            "data-arrow": String(props.arrow),
            "data-disabled": String(props.disabled),
          },
          [
            h(
              "div",
              { class: "mock-mdb-tooltip__reference" },
              slots.reference ? slots.reference() : [],
            ),
            h(
              "div",
              { class: "mock-mdb-tooltip__tip" },
              slots.tip ? slots.tip() : [],
            ),
          ],
        );
    },
  });

  return { MDBTooltip };
});

function mountWithTooltip(bindingValue) {
  const TestComponent = defineComponent({
    props: {
      tooltip: {
        type: String,
        default: "",
      },
    },
    template: '<button id="target" v-mdb-tooltip="tooltip">Target</button>',
  });

  return mount(TestComponent, {
    attachTo: document.body,
    props: {
      tooltip: bindingValue,
    },
    global: {
      directives: {
        "mdb-tooltip": mdbTooltipDirective,
      },
    },
  });
}

function getTooltipNode() {
  return document.body.querySelector(
    ".mdb-tooltip-directive-host .mock-mdb-tooltip",
  );
}

afterEach(() => {
  document.body
    .querySelectorAll(".mdb-tooltip-directive-host")
    .forEach((node) => node.remove());
});

describe("mdb-tooltip directive", () => {
  it("renders default tooltip config for string binding", () => {
    const wrapper = mountWithTooltip("More details");

    const tooltip = getTooltipNode();
    expect(tooltip).not.toBeNull();
    expect(tooltip.getAttribute("data-direction")).toBe("top");
    expect(tooltip.getAttribute("data-max-width")).toBe("320");
    expect(tooltip.getAttribute("data-arrow")).toBe("true");
    expect(tooltip.getAttribute("data-disabled")).toBe("false");
    expect(
      tooltip.querySelector(".mock-mdb-tooltip__tip")?.textContent,
    ).toContain("More details");
    expect(
      wrapper.get("#target").attributes("data-mdb-tooltip-target"),
    ).toMatch(/^mdb-tooltip-target-\d+$/);

    wrapper.unmount();
  });

  it("toggles tooltip visibility on hover", async () => {
    const wrapper = mountWithTooltip("Hover text");
    const target = wrapper.get("#target");

    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("false");

    await target.trigger("mouseenter");
    await nextTick();
    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("true");

    await target.trigger("mouseleave");
    await nextTick();
    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("false");

    wrapper.unmount();
  });

  it("updates tooltip text when binding string changes", async () => {
    const wrapper = mountWithTooltip("Config tooltip");
    expect(
      getTooltipNode()?.querySelector(".mock-mdb-tooltip__tip")?.textContent,
    ).toContain("Config tooltip");

    await wrapper.setProps({ tooltip: "Updated tooltip" });
    await nextTick();

    expect(
      getTooltipNode()?.querySelector(".mock-mdb-tooltip__tip")?.textContent,
    ).toContain("Updated tooltip");

    wrapper.unmount();
  });

  it("disables and hides tooltip when text is empty", async () => {
    const wrapper = mountWithTooltip("Initial value");
    const target = wrapper.get("#target");

    await target.trigger("mouseenter");
    await nextTick();
    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("true");

    await wrapper.setProps({ tooltip: "" });
    await nextTick();

    expect(getTooltipNode()?.getAttribute("data-disabled")).toBe("true");
    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("false");

    await target.trigger("mouseenter");
    await nextTick();
    expect(getTooltipNode()?.getAttribute("data-visible")).toBe("false");

    wrapper.unmount();
  });

  it("cleans up mounted tooltip host on unmount", () => {
    const wrapper = mountWithTooltip("cleanup");
    expect(
      document.body.querySelectorAll(".mdb-tooltip-directive-host"),
    ).toHaveLength(1);

    wrapper.unmount();
    expect(
      document.body.querySelectorAll(".mdb-tooltip-directive-host"),
    ).toHaveLength(0);
  });
});
