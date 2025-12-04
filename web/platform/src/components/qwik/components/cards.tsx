/** @jsxImportSource @builder.io/qwik */
import { type JSXOutput, component$ } from "@builder.io/qwik";

import { LinearGradient } from "./text.tsx";

interface BorderlessCard {
  icon: JSXOutput;
  headline: string;
  text: string;
}

interface VideoCard {
  link: string;
  headline: string;
  description: string;
}

export const BorderlessCard = component$<BorderlessCard>(
  ({ icon, headline, text }) => {
    return (
      <div class="flex h-auto w-5/6 flex-col items-start justify-start md:w-[277px] md:gap-0">
        <div>{icon}</div>
        <LinearGradient
          text={headline}
          class="py-4 pr-8 text-[25px] md:h-[120px] md:px-0 md:py-6"
        />
        <span class="w-full text-[#8280A6] md:w-[273px]">{text}</span>
      </div>
    );
  },
);

export const VideoCard = component$<VideoCard>(
  ({ headline, description, link }) => {
    return (
      <div class="max-w-sm overflow-hidden rounded-xl border border-solid border-primaryBorder text-white shadow-lg">
        <div class="relative h-48 w-full">
          <video
            class="h-full w-full object-cover"
            autoplay={true}
            loop={true}
            muted={true}
          >
            <source src={link} type="video/mp4" />
            Your browser does not support the video tag.
          </video>
        </div>
        <div class="px-6 py-4">
          <h2 class="mb-2 flex h-24 items-center justify-start pr-16 text-xl font-bold">
            <LinearGradient text={headline} />
          </h2>
          <p class="text-base text-gray-400">{description}</p>
        </div>
      </div>
    );
  },
);

const pricing = [
  {
    title: "Open Source",
    items: ["Free!", "Community Support"],
    cta: {
      title: "Get Started",
      link: "/docs/introduction/setup",
    },
  },
  {
    title: "Enterprise",
    items: [
      "Custom pricing",
      "On premise only",
      "Dedicated enterprise support",
    ],
    cta: {
      title: "Request Quote",
      link: "mailto:hello@nativelink.com",
    },
  },
];

export const PricingCard = component$(() => {
  return (
    <div class="flex flex-col md:flex-row items-center justify-center gap-6 md:gap-12">
      {pricing.map((plan) => (
        <div
          id="Card"
          key={plan.title}
          class="flex flex-col gap-5 items-center justify-between p-6 w-80 rounded-lg bg-glass rounded-xl shadow-lg shadow-glass backdrop-blur-[8px] border border-glassBorder"
        >
          <h2 class="text-2xl font-semibold text-white">{plan.title}</h2>
          <ul class="my-4 text-sm text-gray-200 space-y-2">
            {plan.items.map((item) => (
              <li key={item} class="list-disc">
                {item}
              </li>
            ))}
          </ul>
          <a
            href={plan.cta.link}
            class="mt-auto px-4 py-2 bg-blue-500 text-white rounded-lg shadow-md hover:bg-blue-600"
          >
            {plan.cta.title}
          </a>
        </div>
      ))}
    </div>
  );
});
