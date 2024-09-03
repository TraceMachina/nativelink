import { component$ } from "@builder.io/qwik";

import { Benefits } from "../sections/benefits";
import { Community } from "../sections/community";
import { Engineers } from "../sections/engineers";
import { Features } from "../sections/feature";
import { Hero } from "../sections/hero";
import { Testimonial } from "../sections/testimonials";

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
