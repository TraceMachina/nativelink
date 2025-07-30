import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";

export const BookBanner = component$(() => {
  const isMinimized = useSignal(false);
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
      {/* Spacer div to push content down */}
      <div
        class={`transition-all duration-300 ${isMinimized.value ? "h-8" : "h-[52px]"}`}
      />

      {/* Fixed banner */}
      <div
        class={`fixed top-0 left-0 right-0 z-[60] transition-all duration-300 ${
          isMinimized.value ? "transform -translate-y-[calc(100%-2rem)]" : ""
        }`}
      >
        <div class="bg-gradient-to-r from-purple-600 to-blue-600 text-white shadow-lg">
          <div class="container mx-auto px-4">
            <div class="flex items-center justify-between py-3">
              <a
                href="/resources/oreilly-bazel-book"
                class="flex-1 flex items-center justify-center text-center hover:underline"
              >
                <span class="hidden sm:inline mr-2">📘</span>
                <span class="text-sm sm:text-base font-medium">
                  Download our FREE new O'Reilly e-book: Extending Bazel to Its
                  Full Potential!
                </span>
              </a>

              <div class="flex items-center gap-2 ml-4">
                <button
                  type="button"
                  onClick$={() => {
                    isMinimized.value = !isMinimized.value;
                  }}
                  class="p-1 hover:bg-white/20 rounded transition-colors"
                  aria-label={
                    isMinimized.value ? "Expand banner" : "Minimize banner"
                  }
                >
                  <svg
                    class="w-5 h-5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    {isMinimized.value ? (
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M19 9l-7 7-7-7"
                      />
                    ) : (
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
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
                  class="p-1 hover:bg-white/20 rounded transition-colors"
                  aria-label="Close banner"
                >
                  <svg
                    class="w-5 h-5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M6 18L18 6M6 6l12 12"
                    />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>

        {isMinimized.value && (
          <div class="absolute bottom-0 left-0 right-0 bg-gradient-to-r from-purple-700 to-blue-700 h-8 flex items-center justify-center">
            <button
              type="button"
              onClick$={() => {
                isMinimized.value = !isMinimized.value;
              }}
              class="text-white text-xs hover:underline flex items-center gap-1"
            >
              <span>📘 FREE O'Reilly E-Book Available</span>
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
        )}
      </div>
    </>
  );
});
