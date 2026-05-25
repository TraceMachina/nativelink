import { component$ } from "@builder.io/qwik";

const benefits = [
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/robotics.mp4",
    headline: "Written in Rust. Built for scale.",
    description:
      "Memory-safe, race-free, no garbage collector to stall your hot path. Over a billion build requests a month, in production.",
  },
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/video recognition.mp4",
    headline: "Ten minutes to your first cache hit.",
    description:
      "One Docker command. Drops into your existing Bazel, Buck2, Reclient, or CMake setup with zero rewrites.",
  },
  {
    link: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/medicine_tech.mp4",
    headline: "Works with what you've got.",
    description:
      "C++, Rust, Python, Go, and more. Bazel, Buck2, Reclient, and CMake. AWS, GCP, or your own hardware. No lock-in.",
  },
];

export const Benefits = component$(() => {
  return (
    <div class="flex w-full flex-col items-center justify-center gap-16 section-spacing-major section-divider">
      <div class="text-center px-6">
        <h2 class="text-3xl md:text-[58px] font-bold text-black mb-4">
          The NativeLink Difference
        </h2>
      </div>

      {/* Stacked layout with alternating visual rhythm */}
      <div class="max-w-5xl mx-auto px-6 w-full">
        <div class="flex flex-col gap-12">
          {benefits.map((benefit, index) => (
            <div
              key={benefit.link}
              class={`grid md:grid-cols-2 gap-8 items-center ${
                index % 2 === 1 ? "md:grid-flow-dense" : ""
              }`}
            >
              <div class={index % 2 === 1 ? "md:col-start-2" : ""}>
                <div class="relative rounded-[4px] overflow-hidden border-2 border-[rgb(220,220,220)] bg-black">
                  <video
                    class="w-full aspect-video object-cover"
                    autoplay={true}
                    loop={true}
                    muted={true}
                    playsInline={true}
                  >
                    <source src={benefit.link} type="video/mp4" />
                    Your browser does not support the video tag.
                  </video>
                </div>
              </div>
              <div
                class={index % 2 === 1 ? "md:col-start-1 md:row-start-1" : ""}
              >
                <h3 class="text-2xl font-bold text-black mb-4 leading-tight">
                  {benefit.headline}
                </h3>
                <p class="text-lg text-[rgb(60,60,60)] leading-relaxed">
                  {benefit.description}
                </p>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
});
