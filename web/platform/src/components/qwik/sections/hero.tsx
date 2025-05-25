import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";
import { Cloud } from "../../media/icons/icons.tsx";

export const Hero = component$(() => {
  const rotatingText = useSignal("Accelerating Advanced CI");

  useVisibleTask$(() => {
    const options = [
      { text: "Accelerating Advanced CI", duration: 5000 },
      { text: "Remote execution in Bazel", duration: 5000 },
      { text: "Remote execution in Buck2", duration: 5000 },
      { text: "Faster Chromium Builds", duration: 3000 },
      { text: "Model Training on CPUs", duration: 3000 },
      { text: "Robotics", duration: 3000 },
      { text: "Simulation for Semiconductors", duration: 3000 },
    ];

    let index = 0;
    const updateText = () => {
      const option = options[index];
      if (option) {
        rotatingText.value = option.text;
        setTimeout(() => {
          index = (index + 1) % options.length;
          updateText();
        }, option.duration);
      }
    };

    updateText();
  });

  return (
    <div class="relative flex w-full flex-col items-center justify-evenly gap-5 pb-10 text-white overflow-hidden">
      {/* Content */}
      <div class="relative z-20 flex w-full flex-col items-center justify-evenly gap-2 pb-10 pt-36 text-white md:w-[850px]">
        <div class="px-12 md:px-0 md:py-12">
          <div class="flex flex-col items-center gap-2 text-center">
            <h1 class="text-4xl md:text-8xl font-bold">
              The Parallel Compute Platform
            </h1>
            <p class="text-xl md:text-3xl h-[2.5em] transition-opacity duration-500">
              {rotatingText.value}
            </p>
          </div>
        </div>

        <div class="px-8 text-center md:w-[550px]">
          Slash cloud spends. Turbocharge builds.
        </div>

        <div class="w-full p-8 flex flex-row gap-5 justify-center items-center md:flex-row">
          <a
            id="button"
            href="https://app.nativelink.com"
            class=" w-1/2 h-10 gap-2 text-sm flex items-center justify-center rounded-xl border border-solid bg-white text-black transition-all duration-200 hover:border-white hover:bg-black hover:text-white md:h-[37px] md:w-[193px]"
          >
            <Cloud /> Sign up for free
          </a>

          <a
            id="button"
            href="/docs/introduction/setup"
            class="w-1/2 h-10 flex items-center bg-black justify-center rounded-xl border-white/80 border border-solid text-white transition-all duration-200 hover:border-white hover:text-white md:h-[37px] md:w-[193px]"
          >
            Clone the repo
          </a>
        </div>
        {/* <img alt="Nativelink UI" src={MockUp} class="w-[80vw] md:w-full" /> */}
      </div>
    </div>
  );
});
