import { component$ } from "@builder.io/qwik";

import { VideoCard } from "../components/cards";
import { Label } from "../components/text";

const benefits = [
  {
    link: "https://video.wixstatic.com/video/2dfb32_b4d8270ef4ac405fb9284132280261e2/360p/mp4/file.mp4",
    headline: "Made With Love In Rust",
    description:
      "Reduce runtime errors, guarantee memory-safety without requiring garbage collection, & eliminate race conditions at any scale.",
  },
  {
    link: "https://video.wixstatic.com/video/2dfb32_7e9e8ec71c97487089b5f3998cb51515/360p/mp4/file.mp4",
    headline: "Effortless Implementation",
    description:
      "Kickstart NativeLink in 10 minutes with an open-source build cache and remote executor tailored for large code bases",
  },
  {
    link: "https://video.wixstatic.com/video/2dfb32_64737f66368747c9a2d4ce40509d7567/360p/mp4/file.mp4",
    headline: "Universal Language & Platform Compatibility",
    description:
      "Extensive compatibility and support with popular languages (C++, Rust, Python & more), build tools (Bazel, Buck2, & Reclient) and cloud providers (AWS/GCP)",
  },
];

export const Benefits = component$(() => {
  return (
    <div class="flex w-full flex-col items-center justify-center gap-10 py-20">
      <Label text="the nativelink difference" class="text-base px-4" />
      <div class="flex flex-col gap-10 md:flex-row">
        {benefits.map((benefit, _index) => (
          <VideoCard
            key={benefit.link}
            link={benefit.link}
            headline={benefit.headline}
            description={benefit.description}
          />
        ))}
      </div>
    </div>
  );
});
