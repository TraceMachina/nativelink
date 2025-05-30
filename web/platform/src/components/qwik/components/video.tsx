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
      const isMobile = window.innerWidth < 768;
      if (!isMobile && videoElementSignal.value) {
        videoElementSignal.value.src = videoLink;
        videoElementSignal.value.load(); 
        videoElementSignal.value.play().catch((error) => {
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
