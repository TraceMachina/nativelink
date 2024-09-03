import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";
import { scroll } from "motion";

export const ScrollTracker = component$((props: { scrolled: boolean }) => {
  const scrolled = useSignal<boolean>(props.scrolled);

  useVisibleTask$(() => {
    scroll(({ y }) => {
      const wasScrolled = scrolled.value;
      const isScrolled = y.current > 0;

      if (wasScrolled !== isScrolled) {
        scrolled.value = isScrolled;

        // Dispatch custom event with the boolean value
        document.dispatchEvent(
          new CustomEvent("scrolled", { detail: scrolled.value }),
        );
      }
    });
  });

  return <></>;
});
