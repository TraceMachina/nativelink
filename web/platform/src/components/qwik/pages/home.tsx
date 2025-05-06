import { component$ } from "@builder.io/qwik";

import { Benefits } from "../sections/benefits.tsx";
import { Community } from "../sections/community.tsx";
import { Engineers } from "../sections/engineers.tsx";
import { Features } from "../sections/feature.tsx";
import { Hero } from "../sections/hero.tsx";
import { Testimonial } from "../sections/testimonials.tsx";

export const LandingPage = component$(() => {
  return (
    <main class="w-full z-20 bg-black font-nunito text-white">
      <Hero />
      <Testimonial />
      <Features />
      <Engineers />
      <Benefits />
      <Community />
    </main>
  );
});
