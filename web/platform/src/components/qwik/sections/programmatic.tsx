import { component$ } from "@builder.io/qwik";

export const Programmatic = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-8 text-center">
          <p class="text-sm md:text-base uppercase tracking-wide text-[rgb(100,100,100)] font-semibold">
            Programmatic build infrastructure for the agentic era
          </p>

          <h2 class="text-3xl md:text-[58px] font-bold text-black">
            When agents commit code, the build system is the last honest
            checkpoint.
          </h2>

          <div class="text-lg md:text-xl text-[rgb(60,60,60)] leading-relaxed max-w-4xl mx-auto">
            <p>
              AI agents commit faster than humans can review and pull
              dependencies humans never would. NativeLink turns every build into
              structured, observable data — every artifact hashed, every
              dependency traceable, every action programmable.
            </p>
            <p class="mt-4">
              The substrate your security, compliance, and observability tools
              have been waiting for. At agent speed.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});
