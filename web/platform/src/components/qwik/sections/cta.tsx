import { component$ } from "@builder.io/qwik";

export const CTA = component$(() => {
  return (
    <div class="section-spacing-major pb-24">
      <div class="max-w-4xl mx-auto px-6 text-center">
        <h2 class="text-4xl md:text-[58px] font-bold text-black mb-6">
          Try it in 10 minutes.
        </h2>

        <div class="flex flex-col md:flex-row gap-4 justify-center items-center mt-12">
          <a
            href="https://github.com/tracemachina/nativelink"
            target="_blank"
            rel="noreferrer"
            class="w-full md:w-auto h-12 px-8 flex items-center bg-black justify-center border-black border-2 border-solid text-white transition-all duration-200 hover:bg-[rgb(40,40,40)] rounded-interactive font-medium no-underline"
          >
            Clone the repo
          </a>
          <a
            href="https://nativelink.com/cloud"
            class="w-full md:w-auto h-12 px-8 flex items-center bg-transparent justify-center border-black border-2 border-solid text-black transition-all duration-200 hover:bg-[rgb(248,247,244)] rounded-interactive font-medium no-underline"
          >
            Start free
          </a>
          <a
            href="/contact"
            class="w-full md:w-auto h-12 px-8 flex items-center bg-transparent justify-center border-black border-2 border-solid text-black transition-all duration-200 hover:bg-[rgb(248,247,244)] rounded-interactive font-medium no-underline"
          >
            Talk to us
          </a>
        </div>
      </div>
    </div>
  );
});
