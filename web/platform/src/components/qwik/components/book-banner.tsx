import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";

export const BookBanner = component$(() => {
  const isMinimized = useSignal(true);
  const isHidden = useSignal(false);

  // Check localStorage on mount to persist banner state
  useVisibleTask$(() => {
    const bannerState = localStorage.getItem("book-banner-hidden");
    if (bannerState === "true") {
      isHidden.value = true;
    }
  });

  if (isHidden.value) {
    return <div />;
  }

  return (
    <>
      {/* Spacer: matches the visible banner height so page content isn't obscured */}
      <div
        class={`transition-[height] duration-300 ${isMinimized.value ? "h-8" : "h-[44px]"}`}
      />

      {/* Fixed banner — sits above the header (z-[60]) */}
      <div
        class={`fixed top-0 left-0 right-0 z-[60] transition-transform duration-300 ${
          isMinimized.value ? "-translate-y-[calc(100%-2rem)]" : "translate-y-0"
        }`}
      >
        {/* Expanded content */}
        <div class="bg-[#1a1a1a] text-white">
          <div class="max-w-5xl mx-auto px-4">
            <div class="flex items-center justify-between h-11">
              <a
                href="/resources/oreilly-bazel-book"
                class="flex-1 flex items-center justify-center gap-2 text-center no-underline hover:underline"
              >
                <span class="hidden sm:inline text-sm">📘</span>
                <span class="text-xs sm:text-sm font-medium">
                  Download our FREE O&apos;Reilly e-book: Extending Bazel to Its
                  Full Potential
                </span>
                <span class="hidden sm:inline text-xs text-white/60 border border-white/30 rounded px-2 py-0.5 ml-1">
                  Free
                </span>
              </a>

              <div class="flex items-center gap-1 ml-3 shrink-0">
                <button
                  type="button"
                  onClick$={() => {
                    isMinimized.value = !isMinimized.value;
                  }}
                  class="w-7 h-7 flex items-center justify-center rounded hover:bg-white/10 transition-colors"
                  aria-label={
                    isMinimized.value ? "Expand banner" : "Minimise banner"
                  }
                >
                  <svg
                    class="w-3.5 h-3.5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    {isMinimized.value ? (
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2.5"
                        d="M19 9l-7 7-7-7"
                      />
                    ) : (
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2.5"
                        d="M5 15l7-7 7 7"
                      />
                    )}
                  </svg>
                </button>
                <button
                  type="button"
                  onClick$={() => {
                    isHidden.value = true;
                    localStorage.setItem("book-banner-hidden", "true");
                  }}
                  class="w-7 h-7 flex items-center justify-center rounded hover:bg-white/10 transition-colors"
                  aria-label="Close banner"
                >
                  <svg
                    class="w-3.5 h-3.5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2.5"
                      d="M6 18L18 6M6 6l12 12"
                    />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>

        {/* Minimised peek strip */}
        <div class="bg-[#111] h-8 flex items-center justify-center">
          <button
            type="button"
            onClick$={() => {
              isMinimized.value = !isMinimized.value;
            }}
            class="text-white/80 text-xs hover:text-white flex items-center gap-1.5 transition-colors"
          >
            <span>📘 FREE O&apos;Reilly E-Book — Extending Bazel</span>
            <svg
              class="w-3 h-3"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M19 9l-7 7-7-7"
              />
            </svg>
          </button>
        </div>
      </div>
    </>
  );
});
