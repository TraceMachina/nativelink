import { component$ } from "@builder.io/qwik";

interface LinearGradient {
  text: string;
  class?: string;
}

interface Label {
  text: string;
  class?: string;
}

export const LinearGradient = component$<LinearGradient>(
  ({ text, class: customClass = "" }) => {
    return (
      <div
        class={`bg-gradient-to-r from-white to-primary bg-clip-text leading-none tracking-normal text-transparent ${customClass}`}
      >
        {text}
      </div>
    );
  },
);

export const Label = component$<Label>(({ text, class: customClass = "" }) => {
  return (
    <div
      class={`flex items-center justify-center rounded-3xl bg-[#171721] ${customClass}`}
    >
      <div
        class={
          "bg-gradient-to-r from-white to-[#707098] p-2 bg-clip-text text-transparent"
        }
      >
        {text}
      </div>
    </div>
  );
});
