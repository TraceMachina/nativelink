import { component$ } from "@builder.io/qwik";

import { Benefits } from "../sections/benefits.tsx";
import { Contributors } from "../sections/contributors.tsx";
import { CTA } from "../sections/cta.tsx";
import { Features } from "../sections/feature.tsx";
import { Hero } from "../sections/hero.tsx";
import { Industries } from "../sections/industries.tsx";
import { Programmatic } from "../sections/programmatic.tsx";
import { QuickStart } from "../sections/quick-start.tsx";
import { Stats } from "../sections/stats.tsx";
import { Testimonial } from "../sections/testimonials.tsx";

export const LandingPage = component$(() => {
  return (
    <main class="w-full z-20 text-black">
      <Hero />
      <QuickStart />
      <Stats />
      <Testimonial />
      <Contributors />
      <Features />
      <Programmatic />
      <Benefits />
      <Industries />
      <CTA />
    </main>
  );
});
