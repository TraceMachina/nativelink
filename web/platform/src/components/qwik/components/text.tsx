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
      <div class={`text-black leading-none tracking-normal ${customClass}`}>
        {text}
      </div>
    );
  },
);

export const Label = component$<Label>(({ text, class: customClass = "" }) => {
  return (
    <div
      class={`flex items-center justify-center rounded-3xl border border-black/10 bg-white/50 ${customClass}`}
    >
      <div
        class={"p-2 text-sm text-black uppercase tracking-wider font-medium"}
      >
        {text}
      </div>
    </div>
  );
});
