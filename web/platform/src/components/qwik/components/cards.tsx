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
      <div class="flex h-full w-5/6 flex-col items-start justify-start md:w-[277px] p-6">
        <div class="mb-4">{icon}</div>
        <h3 class="text-xl font-bold text-black mb-4 leading-tight">
          {headline}
        </h3>
        <p class="text-base text-[rgb(60,60,60)] leading-relaxed flex-grow">
          {text}
        </p>
      </div>
    );
  },
);

export const VideoCard = component$<VideoCard>(
  ({ headline, description, link }) => {
    return (
      <div class="max-w-sm overflow-hidden rounded-interactive border-2 border-solid border-primaryBorder text-white shadow-lg">
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
          <h3 class="mb-2 flex min-h-24 items-center justify-start text-xl font-bold">
            <LinearGradient text={headline} />
          </h3>
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
    title: "Cloud",
    items: ["$1000/month"],
    cta: {
      title: "Get Started",
      link: "https://dev.nativelink.com/",
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
    <div class="grid md:grid-cols-3 gap-8 max-w-6xl mx-auto">
      {pricing.map((plan) => (
        <div key={plan.title} class="card-warm p-10 flex flex-col gap-6">
          <h2 class="text-3xl font-bold text-black">{plan.title}</h2>
          <ul class="flex-1 space-y-3 text-lg text-[rgb(60,60,60)]">
            {plan.items.map((item) => (
              <li key={item} class="flex items-start gap-3">
                <span class="text-black text-xl flex-shrink-0">•</span>
                <span>{item}</span>
              </li>
            ))}
          </ul>
          <a
            href={plan.cta.link}
            class="mt-auto bg-black text-white rounded-interactive hover:bg-[rgb(40,40,40)] transition-colors duration-200 px-8 min-h-[48px] flex items-center justify-center border-2 border-black font-semibold no-underline"
          >
            {plan.cta.title}
          </a>
        </div>
      ))}
    </div>
  );
});
