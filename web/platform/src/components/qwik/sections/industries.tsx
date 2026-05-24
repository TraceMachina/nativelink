import { component$, useSignal } from "@builder.io/qwik";

interface Industry {
  title: string;
  description: string;
}

const industries: Industry[] = [
  {
    title: "Robotics & Autonomous Systems",
    description:
      "The simulation cycle eats compute. NativeLink caches what's stable, distributes what's new, and gets robotics and AV teams from code change to track-tested in minutes instead of hours.",
  },
  {
    title: "Semiconductors & EDA",
    description:
      "Verification, synthesis, and timing closure devour compute the way C++ builds do — and reward content-addressable caching the same way. NativeLink distributes the workloads chip teams run for weeks at a time, and reuses results across runs, engineers, and projects.",
  },
  {
    title: "Consumer Electronics & Mobile",
    description:
      "Android, embedded firmware, cross-platform clients — millions of build targets across dozens of variants. NativeLink keeps Samsung-scale codebases compilable on a coffee break.",
  },
  {
    title: "Developer Infrastructure & OS",
    description:
      "Linux distributions, security platforms, the systems other systems depend on. NativeLink runs under build pipelines at Rocky Linux (CIQ) and Menlo Security — keeping infrastructure code fast, reproducible, and secure.",
  },
  {
    title: "AI/ML Platforms",
    description:
      "Model code, data pipelines, and inference services rebuild constantly — and increasingly, agents are writing them. NativeLink keeps the loop tight when an AI commits ten times an hour.",
  },
  {
    title: "Browsers & Web Platforms",
    description:
      "Chromium and its descendants — Chrome, Edge, Brave, Electron apps, embedded WebViews — are some of the largest C++ codebases on the open web. NativeLink speaks reclient natively, distributing the millions of compile actions browser teams ship every release cycle.",
  },
  {
    title: "Healthcare & Life Sciences",
    description:
      "Reproducibility isn't optional when an FDA audit asks for byte-identical builds three years later. Hermetic compilation, content-addressed artifacts, end-to-end provenance — by default.",
  },
  {
    title: "Financial Infrastructure",
    description:
      "Latency-critical C++, deterministic builds for compliance, hardware-specific targets for low-latency trading. NativeLink handles all three without the duct tape.",
  },
];

const IndustryItem = component$<{ industry: Industry }>(({ industry }) => {
  const isOpen = useSignal(false);

  return (
    <button
      type="button"
      class="w-full text-left rounded flex justify-between flex-row items-center hover:bg-[rgb(245,245,245)] cursor-pointer transition"
      onClick$={() => {
        isOpen.value = !isOpen.value;
      }}
    >
      <div class="flex flex-col justify-center items-start border-b border-[rgb(220,220,220)] w-full">
        <div class="w-full flex flex-row justify-between py-6">
          <h3 class="text-[46px] font-bold text-black pr-8 leading-tight">
            {industry.title}
          </h3>
          <svg
            width="28px"
            height="30px"
            viewBox="0 0 20 20"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            class={`transition-all duration-500 flex-shrink-0 ${
              isOpen.value ? "rotate-[180deg]" : "rotate-[270deg]"
            }`}
          >
            <title>Toggle industry</title>
            <path
              d="M4.16732 12.5L10.0007 6.66667L15.834 12.5"
              stroke="#000000"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
        <p
          class={`text-lg text-[rgb(60,60,60)] leading-relaxed transition-all duration-400 overflow-hidden ${
            isOpen.value ? "max-h-96 opacity-100 pb-6" : "max-h-0 opacity-0"
          }`}
        >
          {industry.description}
        </p>
      </div>
    </button>
  );
});

export const Industries = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="text-center mb-12">
          <p class="text-sm md:text-base uppercase tracking-wide text-[rgb(100,100,100)] font-semibold mb-4">
            Built for the codebases that shape the physical world
          </p>
          <h2 class="text-3xl md:text-[58px] font-bold text-black mb-6">
            From silicon to simulation, NativeLink runs the builds that ship —
            and the ones agents are writing next.
          </h2>
        </div>

        <div class="flex flex-col">
          {industries.map((industry) => (
            <IndustryItem key={industry.title} industry={industry} />
          ))}
        </div>
      </div>
    </div>
  );
});
