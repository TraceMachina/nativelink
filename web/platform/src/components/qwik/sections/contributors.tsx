import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";

import { Label } from "../components/text.tsx";

import {
  Browserbase,
  Citrix,
  MenloSecurity,
  Meta,
  Samsung,
  Tesla,
  ThirdWave,
} from "../../media/icons/contributors.tsx";

export const Contributors = component$(() => {
  const isMobile = useSignal(false);

  useVisibleTask$(() => {
    isMobile.value = window.innerWidth <= 768;

    const handleResize = () => {
      isMobile.value = window.innerWidth <= 768;
    };

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  });

  const logos_row_width = isMobile.value ? "" : "w-[50vw]"; // Use default width on mobile
  const logo_width = isMobile.value ? "25vw" : "min(10vw, 300px)";
  const logos_top_margin = isMobile.value ? "mt-32" : "mt-16";
  const logos_bottom_margin = isMobile.value ? "-mb-6" : "mb-8";

  const companies = [
    {
      img: <MenloSecurity width={logo_width} />,
    },
    {
      img: <Citrix width={logo_width} />,
    },
    {
      img: <Tesla width={logo_width} />,
    },
    {
      img: <Meta width={logo_width} />,
    },
    {
      img: <Samsung width={logo_width} />,
    },
    {
      img: <ThirdWave width={logo_width} />,
    },
    {
      img: <Browserbase width={logo_width} />,
    },
  ];

  return (
    <div class="flex min-h-fit w-full flex-col items-center justify-center py-16">
      {/* Row 1 - Label */}
      <div class={"flex items-center justify-center"}>
        <Label
          text="Some companies we are building with"
          class={"text-base px-4"}
        />
      </div>

      {/* Row 2 - Empty spacer */}
      <div class="flex h-[5vh] w-full"> </div>

      {/* Row 3 - Display logos */}
      <div
        class={`flex min-h-fit ${logos_row_width} items-center justify-evenly gap-6 flex-wrap ${logos_top_margin} ${logos_bottom_margin}`}
      >
        {companies.map((company) => company.img)}
      </div>
    </div>
  );
});
