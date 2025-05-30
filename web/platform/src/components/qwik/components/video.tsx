import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";

const videoLink =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/background_file.mp4";

interface BackgroundVideoProps {
  class?: string;
}

export const BackgroundVideo = component$<BackgroundVideoProps>(
  ({ class: customClass = "" }) => {
    const videoElementSignal = useSignal<HTMLAudioElement | undefined>();

    useVisibleTask$(() => {
      // This runs only on the client when the component is visible
      const isMobile = window.innerWidth < 768;
      if (!isMobile && videoElementSignal.value) {
        // It's not mobile, and we have the video element!
        // Now, we set the source and tell it to load/play.
        videoElementSignal.value.src = videoLink;
        videoElementSignal.value.load(); // Ensures the video loads now that src is set
        videoElementSignal.value.play().catch((error) => {
          // Autoplay can sometimes fail, log errors if any.
          console.error("Video autoplay failed:", error);
        });
      }
    });
    return (
      <video
        class={`${customClass}`}
        autoplay={true}
        loop={true}
        muted={true}
        ref={videoElementSignal}
        controls={false}
        playsInline={true}
      >
        <source type="video/mp4" />
        Your browser does not support the video tag.
      </video>
    );
  },
);
