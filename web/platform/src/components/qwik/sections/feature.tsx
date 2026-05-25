import { component$ } from "@builder.io/qwik";

import { CAS, RBE, Security } from "../../media/icons/productIcons.tsx";

const products = {
  cas: {
    icon: <CAS />,
    headline: "Faster builds. Lower compute bills.",
    text: "Content-addressable storage means unchanged code never compiles twice — across your team, your CI, and your agents. Every cache hit is a build you don't pay to run.",
  },
  rbe: {
    icon: <RBE />,
    headline: "Scale past one machine. Pay only for what you use.",
    text: "Remote build execution distributes compilation across as many cores as you need — and spins them down when you're done. No idle workstations, no idle workers, no idle bill.",
  },
  security: {
    icon: <Security />,
    headline: "Secure by default.",
    text: "SSO, signed inputs, end-to-end packet integrity. Your source, your artifacts, and your supply chain — locked.",
  },
};

export const Features = component$(() => {
  return (
    <div class="flex flex-col items-center justify-center gap-16 section-spacing-major section-divider">
      <div class="text-center px-6">
        <h2 class="text-3xl md:text-[58px] font-bold text-black mb-4">
          What You Get
        </h2>
      </div>

      {/* Asymmetric 2-up layout - eliminates AI template pattern */}
      <div class="max-w-6xl mx-auto px-6 w-full">
        <div class="grid md:grid-cols-2 gap-8 items-start">
          {/* Left: Primary feature with larger visual weight */}
          <div class="flex flex-col gap-12">
            <div class="flex items-start gap-6">
              <div class="flex-shrink-0 mt-1">{products.cas.icon}</div>
              <div>
                <h3 class="text-2xl font-bold text-black mb-4 leading-tight">
                  {products.cas.headline}
                </h3>
                <p class="text-lg text-[rgb(60,60,60)] leading-relaxed">
                  {products.cas.text}
                </p>
              </div>
            </div>

            <div class="flex items-start gap-6">
              <div class="flex-shrink-0 mt-1">{products.security.icon}</div>
              <div>
                <h3 class="text-2xl font-bold text-black mb-4 leading-tight">
                  {products.security.headline}
                </h3>
                <p class="text-lg text-[rgb(60,60,60)] leading-relaxed">
                  {products.security.text}
                </p>
              </div>
            </div>
          </div>

          {/* Right: Feature with emphasis */}
          <div class="bg-[rgb(248,247,244)] border-2 border-[rgb(220,220,220)] rounded-[4px] p-10 flex flex-col">
            <div class="mb-6">{products.rbe.icon}</div>
            <h3 class="text-3xl font-bold text-black mb-6 leading-tight">
              {products.rbe.headline}
            </h3>
            <p class="text-xl text-[rgb(60,60,60)] leading-relaxed">
              {products.rbe.text}
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});
