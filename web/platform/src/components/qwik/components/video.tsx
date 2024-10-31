import { component$, useSignal } from "@builder.io/qwik";

const videoLink =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/background_file.mp4";

interface BackgroundVideoProps {
  class?: string;
}

export const BackgroundVideo = component$<BackgroundVideoProps>(
  ({ class: customClass = "" }) => {
    const videoElementSignal = useSignal<HTMLAudioElement | undefined>();

    return (
      <video
        class={`${customClass}`}
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
    );
  },
);
