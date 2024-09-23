import { component$, useSignal } from "@builder.io/qwik";

const videoLink =
  "https://video.wixstatic.com/video/e16379_17fe40ef02fe478fb675cced13e69dde/720p/mp4/file.mp4";

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
