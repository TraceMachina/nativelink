import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";
import { Background, Cloud } from "../../media/icons/icons.tsx";
import { LinearGradient } from "../components/text.tsx";
import { BackgroundVideo } from "../components/video.tsx";

const _videoLink =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/background_file.mp4";
const _MockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_dashboard.webp";
const videoMockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_introduction.mp4";

export const Hero = component$(() => {
  const rotatingText = useSignal("Accelerating Advanced CI");
  const videoElementSignal = useSignal<HTMLAudioElement | undefined>();

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

  const _videoElementSignal = useSignal<HTMLAudioElement | undefined>();

  return (
    <div class="relative flex w-full flex-col items-center justify-evenly gap-5 pb-10 text-white overflow-hidden">
      {/* Background Video */}
      <BackgroundVideo class="absolute top-10 md:top-[-20vh] left-0 z-0 w-full object-cover" />

      <div class="absolute md:hidden flex justify-center items-center overflow-hidden">
        <Background class="w-[200vw]" />
      </div>
      <div class="absolute hidden md:flex justify-center items-center overflow-hidden">
        <div class="bg-black/50 w-screen h-screen" />
      </div>

      {/* Overlay Image */}
      {/* <img
        src={Overlay.src}
        class="absolute left-0 right-0 top-0 z-10 mx-auto h-auto w-full object-cover"
        alt="Overlay"
      /> */}

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
        <div class="w-full flex justify-center items-center">
          <div class="w-9/11 aspect-video relative">
            <video
              src={videoMockUp}
              class="w-full h-full object-contain self-center shadow-[0px_0px_50px_0px_rgba(96,80,230,0.3)]"
              autoplay={false}
              loop={true}
              muted={true}
              poster={_MockUp}
              controls={true}
              preload="metadata"
            />
          </div>
        </div>
        {/* <img alt="Nativelink UI" src={MockUp} class="w-[80vw] md:w-full" /> */}
      </div>
    </div>
  );
});
