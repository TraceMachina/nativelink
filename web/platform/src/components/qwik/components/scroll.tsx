import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";
import { scroll } from "motion";

export const ScrollTracker = component$((props: { scrolled: boolean }) => {
  const scrolled = useSignal<boolean>(props.scrolled);

  useVisibleTask$(() => {
    const cleanup = scroll((progress) => {
      // Progress is a value between 0 and 1
      // Consider scrolled if we're even slightly scrolled down
      const isScrolled = progress > 0;

      if (scrolled.value !== isScrolled) {
        scrolled.value = isScrolled;
        document.dispatchEvent(
          new CustomEvent("scrolled", { detail: scrolled.value }),
        );
      }
    });

    return () => {
      if (typeof cleanup === "function") {
        cleanup();
      }
    };
  });

  return <></>;
});
