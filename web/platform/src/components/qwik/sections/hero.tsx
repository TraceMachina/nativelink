import { component$, useSignal } from "@builder.io/qwik";
import { Background, Cloud } from "../../media/icons/icons";
import { LinearGradient } from "../components/text";

const videoLink =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/background_file.mp4";
const _MockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_dashboard.webp";
const videoMockUp =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_background.mp4";

export const Hero = component$(() => {
  const videoElementSignal = useSignal<HTMLAudioElement | undefined>();
  return (
    <div class="relative flex w-full flex-col items-center justify-evenly gap-5 pb-10 text-white overflow-hidden">
      {/* Background Video */}
      <video
        class="absolute top-10 md:top-[-20vh] left-0 z-0 w-full object-cover"
        autoplay={true}
        loop={true}
        muted={true}
        ref={videoElementSignal}
        controls={false}
        src={videoLink}
        playsInline={true}
      >
        <source type="video/mp4" />
        Your browser does not support the video tag.
      </video>

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
          <LinearGradient
            text="The simulation infrastructure platform"
            class="text-3xl md:text-7xl text-center"
          />
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
          <video
            src={videoMockUp}
            class="w-9/11 self-center shadow-[0px_0px_50px_0px_rgba(96,80,230,0.3)]"
            autoplay={false}
            loop={true}
            muted={true}
            poster={_MockUp}
            controls={true}
          />
        </div>
        {/* <img alt="Nativelink UI" src={MockUp} class="w-[80vw] md:w-full" /> */}
      </div>
    </div>
  );
});
