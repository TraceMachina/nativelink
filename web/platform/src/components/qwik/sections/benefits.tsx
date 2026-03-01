import { component$ } from "@builder.io/qwik";

import { VideoCard } from "../components/cards.tsx";
import { Label } from "../components/text.tsx";

const benefits = [
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/robotics.mp4",
    headline: "Made With Love In Rust",
    description:
      "Reduce runtime errors, guarantee memory-safety without requiring garbage collection, & eliminate race conditions at any scale.",
  },
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/video recognition.mp4",
    headline: "Effortless Implementation",
    description:
      "Kickstart NativeLink in 10 minutes with an open-source build cache and remote executor tailored for large code bases",
  },
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/medicine_tech.mp4",
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
