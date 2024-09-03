import { component$, useVisibleTask$ } from "@builder.io/qwik";

export const ProductPage = component$(() => {
  useVisibleTask$(() => {
    console.info("You can find more about the NativeLink Products here.");
  });

  return (
    <main class="flex w-full flex-col items-center justify-center gap-10 bg-black font-nunito text-white">
      <div class="flex justify-center flex-col items-center w-screen h-screen text-white text-4xl ">
        Products Page
      </div>
    </main>
  );
});
