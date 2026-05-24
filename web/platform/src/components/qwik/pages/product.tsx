import { component$ } from "@builder.io/qwik";

const videoMockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/Walkthrough+of+Nativelink+Cloud.mp4";

const _MockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_dashboard.webp";

const ProductHero = component$(() => {
  return (
    <div class="relative flex w-full flex-col items-center justify-evenly gap-5 pb-10 text-black overflow-hidden">
      <div class="relative z-20 flex w-full flex-col items-center justify-evenly gap-2 pb-10 pt-36 text-black md:w-[900px]">
        <div class="px-12 md:px-0 md:py-12">
          <div class="flex flex-col items-center gap-6 text-center">
            <p class="text-sm md:text-base uppercase tracking-wide text-[rgb(100,100,100)] font-semibold">
              Product
            </p>
            <h1 class="text-4xl md:text-7xl font-bold leading-tight">
              One platform. Every build. Every machine.
            </h1>
            <p class="text-lg md:text-xl text-[rgb(60,60,60)] max-w-[800px] leading-relaxed">
              NativeLink unifies remote caching, remote execution, and
              observability into a single Rust-native platform — built to keep
              up with codebases that grow faster than you can provision them.
            </p>
          </div>
        </div>

        <div class="w-full p-8 flex flex-col md:flex-row gap-4 justify-center items-center">
          <a
            href="/docs/introduction/setup"
            class="w-full md:w-auto h-12 px-8 flex items-center bg-black justify-center border-black border-2 border-solid text-white transition-all duration-200 hover:bg-[rgb(40,40,40)] rounded-interactive font-medium no-underline"
          >
            Start in 10 minutes
          </a>
          <a
            href="/contact"
            class="w-full md:w-auto h-12 px-8 flex items-center bg-transparent justify-center border-black border-2 border-solid text-black transition-all duration-200 hover:bg-[rgb(248,247,244)] rounded-interactive font-medium no-underline"
          >
            Talk to us
          </a>
        </div>

        <div class="w-full flex justify-center items-center">
          <div class="w-9/11 relative">
            <video
              src={videoMockUp}
              class="w-full h-full object-contain self-center shadow-[0px_0px_50px_0px_rgba(96,80,230,0.3)] border-2 border-[rgb(220,220,220)] rounded-interactive"
              autoplay={false}
              loop={true}
              muted={true}
              poster={_MockUp}
              controls={true}
              preload="metadata"
            />
          </div>
        </div>
      </div>
    </div>
  );
});

const RemoteCache = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            Remote Cache
          </h2>
          <div class="text-2xl md:text-3xl font-semibold text-black">
            Cache once. Reuse forever.
          </div>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              Content-addressable storage deduplicates every artifact your team
              produces. If a teammate, your CI, or an agent has already built
              it, you get it back in milliseconds. Drops into Bazel, Buck2,
              Reclient, Pants, Goma — and CMake via recc.
            </p>
            <p class="mt-4 text-sm italic">
              Proof point: Over a billion build requests served per month, in
              production.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});

const RemoteExecution = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            Remote Build Execution
          </h2>
          <div class="text-2xl md:text-3xl font-semibold text-black">
            Distribute across every core you have.
          </div>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              Offload compilation and tests to a worker fleet that scales
              horizontally — on AWS, GCP, or bare metal. Hermetic by design,
              deterministic by default. Specialized hardware (GPUs, ARM, Apple
              Silicon) supported natively.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});

const CloudAndSelfHost = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            Cloud & Self-Host
          </h2>
          <div class="text-2xl md:text-3xl font-semibold text-black">
            Hosted by us. Or run by you.
          </div>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              Start free on NativeLink Cloud in ten minutes. Move to dedicated
              infrastructure when your scale demands it. Or self-host the
              open-source release with one Docker command. Same code path. Same
              performance.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});

const BuiltOnRust = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            Built on Rust
          </h2>
          <div class="text-2xl md:text-3xl font-semibold text-black">
            Performance that doesn't stall.
          </div>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              No garbage collector. No race conditions at scale. No mystery
              latency spikes. Memory safety without the runtime tax — which is
              why NativeLink can serve a billion requests a month on
              infrastructure that would buckle other systems.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});

const SecurityAndProvenance = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            Security & Programmable Provenance
          </h2>
          <div class="text-2xl md:text-3xl font-semibold text-black">
            Trust at every hop. Provenance at every step.
          </div>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              SSO/SAML, signed worker inputs and outputs, packet integrity,
              end-to-end TLS. Build artifacts are content-addressed and
              tamper-evident by design. Hermetic execution means no surprise
              dependencies pulled in mid-build — every input explicit, every
              output verifiable.
            </p>
            <p class="mt-4">
              And because builds are programmable, your security and
              observability tools can plug straight into the data: dependency
              graphs, execution metadata, artifact provenance — queryable,
              exportable, auditable. Critical when humans are committing code.
              Existential when agents are.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
});

const Integrations = component$(() => {
  const integrations = [
    {
      title: "AI Coding Platforms",
      items:
        "Claude Code, GitHub Copilot Workspace, Devin, Cursor, Windsurf, and more.",
    },
    {
      title: "Languages",
      items: "C++, Rust, Python, Go, Java, Kotlin, Swift, and more.",
    },
    {
      title: "Build systems",
      items: "Bazel, Buck2, Reclient, Soong, Pants, Goma — and CMake via recc.",
    },
    {
      title: "Cloud",
      items: "AWS, GCP, and Azure.",
    },
    {
      title: "CI",
      items: "GitHub Actions, GitLab CI, Buildkite, Jenkins.",
    },
  ];

  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <h2 class="text-3xl md:text-5xl font-bold text-black mb-12">
          Integrations
        </h2>
        <div class="text-2xl md:text-3xl font-semibold text-black mb-8">
          Works with your stack.
        </div>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-8">
          {integrations.map((integration) => (
            <div key={integration.title} class="flex flex-col gap-2">
              <div class="text-xl font-bold text-black">
                {integration.title}
              </div>
              <div class="text-base text-[rgb(60,60,60)]">
                {integration.items}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
});

const CaseStudy = component$(() => {
  return (
    <div class="section-spacing-major section-divider">
      <div class="max-w-5xl mx-auto px-6">
        <div class="flex flex-col gap-6">
          <p class="text-sm md:text-base uppercase tracking-wide text-[rgb(100,100,100)] font-semibold">
            Proof at scale
          </p>
          <h2 class="text-3xl md:text-5xl font-bold text-black">
            LLVM builds 4x faster on NativeLink.
          </h2>
          <div class="text-lg text-[rgb(60,60,60)] leading-relaxed">
            <p>
              LLVM contributors are using NativeLink with CMake and recc to
              distribute builds of clang and the LLVM toolchain — cutting
              full-project compile time from 17 minutes to 4. No build-system
              migration. No proprietary client. Just your existing CMake setup,
              pointed at NativeLink.
            </p>
          </div>
          <div class="mt-4">
            <a
              href="https://reidkleckner.dev/posts/llvm-recc-nativelink/"
              target="_blank"
              rel="noreferrer"
              class="inline-flex items-center text-black font-medium hover:opacity-70 transition-opacity"
            >
              Read the writeup →
            </a>
          </div>
        </div>
      </div>
    </div>
  );
});

const ProductCTA = component$(() => {
  return (
    <div class="section-spacing-major pb-24">
      <div class="max-w-4xl mx-auto px-6 text-center">
        <h2 class="text-4xl md:text-6xl font-bold text-black mb-6">
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

export const ProductPage = component$(() => {
  return (
    <main class="w-full z-20 text-black">
      <ProductHero />
      <RemoteCache />
      <RemoteExecution />
      <CloudAndSelfHost />
      <BuiltOnRust />
      <SecurityAndProvenance />
      <Integrations />
      <CaseStudy />
      <ProductCTA />
    </main>
  );
});
