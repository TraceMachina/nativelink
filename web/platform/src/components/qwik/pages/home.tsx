import { component$ } from "@builder.io/qwik";

import { Benefits } from "../sections/benefits.tsx";
import { Community } from "../sections/community.tsx";
import { Customers } from "../sections/customers.tsx";
import { Features } from "../sections/feature.tsx";
import { Hero } from "../sections/hero.tsx";
import { Testimonial } from "../sections/testimonials.tsx";

export const LandingPage = component$(() => {
  return (
    <main class="w-full z-20 bg-black font-nunito text-white">
      <Hero />
      <Customers />
      <Testimonial />
      <Features />
      <Benefits />
      <Community />
    </main>
  );
});
