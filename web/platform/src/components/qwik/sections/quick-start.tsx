import { component$ } from "@builder.io/qwik";
import { AnimatedTerminal } from "../components/animated-terminal.tsx";

export const QuickStart = component$(() => {
  return (
    <div class="relative w-full py-24 px-6">
      {/* Gradient background similar to mockup */}
      <div class="absolute inset-0 bg-gradient-to-br from-[#1a1a2e] via-[#16213e] to-[#0f0f23]" />

      <div class="relative z-10 max-w-7xl mx-auto">
        <div class="grid md:grid-cols-2 gap-12 items-center">
          {/* Left: Terminal */}
          <div>
            <AnimatedTerminal />
          </div>

          {/* Right: Copy */}
          <div class="text-white space-y-6">
            <h2 class="text-4xl md:text-[58px] font-bold leading-tight">
              Built for AI-Assisted Development
            </h2>
            <p class="text-lg md:text-xl text-gray-300 leading-relaxed">
              Start in 10 minutes with Docker. Develop with Claude Code skills
              that guide you through storage changes, config updates, and
              debugging — the same workflows NativeLink engineers use in
              production.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});
