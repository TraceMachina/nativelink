import { component$ } from "@builder.io/qwik";

export const Hero = component$(() => {
  return (
    <div class="relative flex w-full flex-col items-center justify-evenly gap-5 pb-16 text-black overflow-hidden">
      {/* Content — pt accounts for: 32px minimised banner + 56px header + 32px breathing room */}
      <div class="relative z-20 flex w-full flex-col items-center justify-evenly gap-4 pb-10 pt-[152px] text-black md:w-[860px] px-6 md:px-0">
        <div class="flex flex-col items-center gap-5 text-center">
          <p class="text-xs md:text-sm uppercase tracking-widest text-[rgb(110,110,110)] font-semibold animate-fade-in">
            Build infrastructure for the agentic era
          </p>
          <h1 class="text-[2.5rem] md:text-[4rem] lg:text-[4.5rem] font-bold leading-[1.1] tracking-tight animate-fade-in-up animation-delay-100">
            When agents write your code, your build system is the bottleneck.
          </h1>
          <p class="text-base md:text-lg text-[rgb(65,65,65)] max-w-[680px] leading-relaxed animate-fade-in-up animation-delay-200">
            NativeLink is the parallel compute platform that keeps builds fast
            while your codebase — and your agents — multiply. Rust-powered. Open
            source. Trusted in production by thousands of developers.
          </p>
        </div>

        <div class="w-full pt-4 flex flex-col md:flex-row gap-3 justify-center items-center animate-fade-in-up animation-delay-300">
          <a
            id="button"
            href="https://github.com/tracemachina/nativelink"
            target="_blank"
            rel="noreferrer"
            class="w-full md:w-auto h-11 px-7 flex items-center bg-black justify-center text-white transition-colors duration-200 hover:bg-[rgb(30,30,30)] rounded-lg font-medium text-sm"
          >
            Clone the repo
          </a>
          <a
            href="/docs/introduction/setup#-quickstart"
            class="w-full md:w-auto h-11 px-7 flex items-center bg-transparent justify-center border border-[rgb(180,180,180)] text-black transition-colors duration-200 hover:border-black rounded-lg font-medium text-sm"
          >
            Start free
          </a>
          <a
            href="/contact"
            class="w-full md:w-auto h-11 px-7 flex items-center bg-transparent justify-center text-[rgb(80,80,80)] transition-colors duration-200 hover:text-black rounded-lg font-medium text-sm underline-offset-4"
          >
            Talk to us
          </a>
        </div>

        <div class="w-full pt-6 flex justify-center items-center animate-fade-in animation-delay-400">
          <div class="w-full md:w-10/11 relative aspect-video">
            <iframe
              class="absolute top-0 left-0 w-full h-full rounded-xl border border-[rgb(210,210,210)] shadow-[0_8px_40px_rgba(124,58,237,0.18)]"
              src="https://www.youtube.com/embed/f7kR1woFqcU"
              title="NativeLink Demo"
              allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
              allowFullscreen={true}
            />
          </div>
        </div>
      </div>
    </div>
  );
});
