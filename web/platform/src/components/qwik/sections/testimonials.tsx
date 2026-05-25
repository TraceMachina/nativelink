import {
  type QwikIntrinsicElements,
  _jsxQ,
  _jsxS,
  component$,
} from "@builder.io/qwik";

const SiRockylinux = (props: QwikIntrinsicElements["svg"]) =>
  /* @__PURE__ */ _jsxS(
    "svg",
    {
      ...props,
      children: [
        /* @__PURE__ */ _jsxQ("title", null, null, "Rocky Linux", 3, null),
        /* @__PURE__ */ _jsxQ(
          "path",
          null,
          {
            d: "M23.332 15.957c.433-1.239.668-2.57.668-3.957 0-6.627-5.373-12-12-12S0 5.373 0 12c0 3.28 1.315 6.251 3.447 8.417L15.62 8.245l3.005 3.005zm-2.192 3.819l-5.52-5.52L6.975 22.9c1.528.706 3.23 1.1 5.025 1.1 3.661 0 6.94-1.64 9.14-4.224z",
          },
          null,
          3,
          null,
        ),
      ],
    },
    {
      "data-qwikest-icon": true,
      fill: "currentColor",
      height: "1em",
      role: "img",
      stroke: "none",
      viewBox: "0 0 24 24",
      width: "1em",
      xmlns: "http://www.w3.org/2000/svg",
    },
    0,
    "4I_0",
  );

export const Testimonial = component$(() => {
  return (
    <div class="flex flex-col gap-10 flex justify-center items-center section-spacing-minor section-divider">
      <div class="w-full px-14 py-6 text-center text-lg leading-normal tracking-normal text-black md:w-[891px] md:px-0 md:text-justify md:text-[28px] border-l-4 border-black">
        "Running NativeLink in production with great results. Great work folks."
      </div>
      <div class="flex flex-row items-center justify-center gap-2 text-black">
        <span>Mustafa Gezen</span>
        <span class="text-gray-600">—</span>
        <div class="flex flex-row justify-center items-center gap-2">
          <SiRockylinux class="fill-[#10B981] w-8 h-8" />
          <span>
            <strong>Rocky</strong> Linux
          </span>
        </div>
      </div>
    </div>
  );
});
