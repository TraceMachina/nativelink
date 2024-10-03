import { $, component$, useOnDocument, useSignal } from "@builder.io/qwik";

export const Island = component$((props: { initialVal: number }) => {
  const count = useSignal<number>(props.initialVal);

  useOnDocument(
    "counter",
    $((e: CustomEvent<number>) => {
      count.value = e.detail;
    }),
  );

  return <p class="text-white">Message received: {count.value}</p>;
});
