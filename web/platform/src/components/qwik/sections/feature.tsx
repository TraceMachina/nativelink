import { component$ } from "@builder.io/qwik";

import { CAS, FreeCloud, RBE, Security } from "../../media/icons/productIcons";

import { BorderlessCard } from "../components/cards";
import { Label } from "../components/text";

const products = [
  {
    icon: <CAS />,
    headline: "Content Addressable Storage (CAS)",
    text: "Minimize redundant compilation of unchanged source code for compute-efficient builds",
  },
  {
    icon: <Security />,
    headline: "Security (SSO, Packet Integrity)",
    text: "Authenticate users/workers input, and outputs to guarantee secure access to projects",
  },

  {
    icon: <RBE />,
    headline: 'Remote Build Execution (RBE) "Preview"',
    text: "Incorporate multi-core distribution for project builds to boost efficiency and speed up development cycles",
  },

  {
    icon: <FreeCloud />,
    headline: "Free Cloud",
    text: "Fully managed simulation runtime",
  },
];

export const Features = component$(() => {
  return (
    <div class="flex flex-col items-center justify-center gap-24">
      <Label text="a seamless experience" class="text-base px-4" />
      <div class="flex flex-col items-center gap-12 md:flex-row md:items-start md:justify-evenly md:gap-3">
        {products.map((product, _index) => (
          <BorderlessCard
            key={product.headline}
            icon={product.icon}
            headline={product.headline}
            text={product.text}
          />
        ))}
      </div>
      <button
        type="button"
        class="flex items-center justify-center rounded-[14px] border border-solid border-transparent bg-gradient-to-r from-[#724FBA] to-[#292C9F] px-12 py-2 text-white transition-all duration-200 hover:border-white hover:bg-black hover:text-white md:h-[37px] md:px-5"
      >
        Demo now
      </button>
    </div>
  );
});
