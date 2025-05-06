import { _jsxQ, _jsxS, component$ } from "@builder.io/qwik";

const HiCheckSolid = (props) =>
  /* @__PURE__ */ _jsxS(
    "svg",
    {
      ...props,
      children: /* @__PURE__ */ _jsxQ(
        "path",
        null,
        {
          "clip-rule": "evenodd",
          d: "M19.916 4.626a.75.75 0 01.208 1.04l-9 13.5a.75.75 0 01-1.154.114l-6-6a.75.75 0 011.06-1.06l5.353 5.353 8.493-12.739a.75.75 0 011.04-.208z",
          "fill-rule": "evenodd",
        },
        null,
        3,
        null,
      ),
    },
    {
      "aria-hidden": "true",
      "data-qwikest-icon": true,
      fill: "currentColor",
      height: "1em",
      viewBox: "0 0 24 24",
      width: "1em",
      xmlns: "http://www.w3.org/2000/svg",
    },
    0,
    "ba_0",
  );

export const Checkmark = component$(() => {
  return (
    <div class="flex justify-center items-center">
      <HiCheckSolid class="text-2xl" style={{ color: "green" }} />
    </div>
  );
});

const HiXMarkSolid = (props) =>
  /* @__PURE__ */ _jsxS(
    "svg",
    {
      ...props,
      children: /* @__PURE__ */ _jsxQ(
        "path",
        null,
        {
          "clip-rule": "evenodd",
          d: "M5.47 5.47a.75.75 0 011.06 0L12 10.94l5.47-5.47a.75.75 0 111.06 1.06L13.06 12l5.47 5.47a.75.75 0 11-1.06 1.06L12 13.06l-5.47 5.47a.75.75 0 01-1.06-1.06L10.94 12 5.47 6.53a.75.75 0 010-1.06z",
          "fill-rule": "evenodd",
        },
        null,
        3,
        null,
      ),
    },
    {
      "aria-hidden": "true",
      "data-qwikest-icon": true,
      fill: "currentColor",
      height: "1em",
      viewBox: "0 0 24 24",
      width: "1em",
      xmlns: "http://www.w3.org/2000/svg",
    },
    0,
    "YY_0",
  );

export const Xmark = component$(() => {
  return (
    <div class="flex justify-center items-center">
      <HiXMarkSolid class="text-2xl" style={{ color: "red" }} />
    </div>
  );
});
