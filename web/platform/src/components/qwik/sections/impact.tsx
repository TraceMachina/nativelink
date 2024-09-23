import { component$ } from "@builder.io/qwik";

export const Impact = component$(() => {
  return (
    <div class="h-[650px] w-full flex-col  gap-10">
      <div class="relative flex h-full w-full items-center justify-center overflow-hidden">
        <video
          autoplay={true}
          loop={true}
          muted={true}
          playsInline={true}
          class="absolute left-1/2 top-1/2 h-full w-full -translate-x-1/2 -translate-y-1/2 transform object-cover"
        >
          {/* <source src="/videos/video_2.mp4" type="video/mp4" /> */}
          Your browser does not support the video tag.
        </video>
        <div class="relative z-10 flex h-32 flex-col items-center justify-center gap-4 md:flex-row">
          <h2 class="flex h-full items-center justify-center bg-gradient-to-r from-white to-[#707098] bg-clip-text text-4xl  text-[55px] text-transparent">
            Saving Lives.
          </h2>
          <h2 class="flex h-full items-center justify-center bg-gradient-to-r from-white to-[#707098] bg-clip-text text-4xl  text-[55px] text-transparent">
            Saving Time.
          </h2>
          <h2 class="flex h-full items-center justify-center bg-gradient-to-r from-white to-[#707098] bg-clip-text text-4xl  text-[55px] text-transparent">
            Saving Money.
          </h2>
        </div>
      </div>
    </div>
  );
});
