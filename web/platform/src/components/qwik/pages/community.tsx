import { component$ } from "@builder.io/qwik";

import { GitHub, Slack } from "../../media/icons/icons.tsx";

export const CommunityPage = component$(() => {
  return (
    <main class="bg-[rgb(248,247,244)] min-h-screen">
      {/* Hero Section */}
      <div class="section-spacing-major section-divider">
        <div class="max-w-6xl mx-auto px-6">
          <h1 class="text-5xl md:text-7xl font-bold text-black mb-8">
            Join our community
          </h1>

          {/* Connect Cards */}
          <div class="grid md:grid-cols-3 gap-8 pt-20 md:pt-24">
            <a
              href="/docs/introduction/setup"
              aria-label="Read the Docs"
              class="card-warm p-8 flex flex-col gap-6 hover:-translate-y-1 transition-all duration-200 no-underline items-center text-center"
            >
              <svg
                class="w-16 h-16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="rgb(100,100,255)"
                stroke-width="2"
                aria-hidden="true"
                focusable="false"
              >
                <rect x="2" y="3" width="20" height="18" rx="2" />
                <path d="M8 7h8M8 11h8M8 15h4" />
              </svg>
              <h3 class="text-2xl font-bold text-black">Read the Docs</h3>
            </a>

            <a
              href="https://forms.gle/LtaWSixEC6bYi5xF7"
              aria-label="Join our Slack"
              class="card-warm p-8 flex flex-col gap-6 hover:-translate-y-1 transition-all duration-200 no-underline items-center text-center"
            >
              <div class="flex-shrink-0">
                <Slack />
              </div>
              <h3 class="text-2xl font-bold text-black">Join our Slack</h3>
            </a>

            <a
              href="https://github.com/tracemachina/nativelink"
              aria-label="Clone the Repo"
              class="card-warm p-8 flex flex-col gap-6 hover:-translate-y-1 transition-all duration-200 no-underline items-center text-center"
            >
              <div class="flex-shrink-0">
                <GitHub />
              </div>
              <h3 class="text-2xl font-bold text-black">Clone the Repo</h3>
            </a>
          </div>
        </div>
      </div>

      {/* Events section intentionally hidden. */}

      {/* Appreciation Section */}
      <div class="section-spacing-major">
        <div class="max-w-3xl mx-auto px-6 text-center">
          <h2 class="text-3xl md:text-5xl font-bold text-black mb-6">
            We Love Our Community
          </h2>
          <p class="text-lg text-[rgb(60,60,60)] leading-relaxed mb-8">
            To say thank you to our NativeLink contributors and community
            members, we’d love to send you something special as a small token of
            appreciation. Simply fill out the form below, our team will verify
            your details, and send something special your way.
          </p>
          <a
            href="https://forms.gle/LtaWSixEC6bYi5xF7"
            class="inline-flex bg-black text-white hover:bg-[rgb(40,40,40)] transition-colors duration-200 px-12 min-h-[48px] rounded-interactive justify-center items-center border-2 border-black font-semibold"
          >
            Fill out the form
          </a>
        </div>
      </div>
    </main>
  );
});
