import { component$, useVisibleTask$ } from "@builder.io/qwik";

import { Label, LinearGradient } from "../components/text.tsx";

import { BackgroundVideo } from "../components/video.tsx";

const team = [
  {
    img: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/aaron_mondal.webp",
    name: "Aaron Mondal",
    title: "Co-Founder & Software Engineer",
  },
  {
    img: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/tim_potter.webp",
    name: "Tim Potter",
    title: "Chief Technology Officer",
  },
  {
    img: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/tyrone_greenfield.webp",
    name: "Tyrone Greenfield",
    title: "Chief of Staff",
  },
  {
    img: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/marcus_eagan.webp",
    name: "Marcus Eagan",
    title: "Co-Founder and Janitor",
  },
];

const _cards = [
  {
    title: "Media Kit",
    description:
      "Download our media kit for logos, branding guidelines, and other resources.",
    linkTitle: "Download",
    link: "https://www.google.com/url?q=https://www.google.com/url?q%3Dhttps://drive.google.com/drive/folders/1fNmr4IpIaaiej6yKr1t0ispbXOaCVVTa?usp%253Dsharing%26amp;sa%3DD%26amp;source%3Deditors%26amp;ust%3D1723017295224121%26amp;usg%3DAOvVaw2K4tnmOdf37pPSQpeq0YIG&sa=D&source=docs&ust=1723017295247588&usg=AOvVaw1NaQYPBuTBwCIVNd6KkL0l",
  },
  {
    title: "General inquiries",
    description: "Reach out to us with any questions or comments you may have.",
    linkTitle: "Contact",
    link: "mailto:contact@nativelink.com",
  },
  {
    title: "Sales inquiries",
    description:
      "Interested in learning more about NativeLink?  Contact our sales team for more information.",
    linkTitle: "Contact",
    link: "mailto:contact@nativelink.com",
  },
];

const CompanyHero = component$(() => {
  return (
    <div class="relative w-full flex flex-col md:flex-row  md:gap-14 justify-center items-center ">
      <BackgroundVideo class="z-0 absolute inset-0 w-full h-full object-cover" />
      <div class="absolute inset-0 w-full h-full bg-gradient-to-b from-black via-black/40 to-black opacity-80" />

      <div class="px-8 z-10 gap-10 flex flex-col md:w-[650px] ">
        <div class="pr-8">
          <LinearGradient
            text="NativeLink is built by Trace Machina"
            class="text-4xl md:text-5xl text-left pr-10"
          />
        </div>
        <span class="text-white! test-base">
          At Trace Machina, our mission is to revolutionize the development of
          complex systems by providing cutting-edge tools that drastically
          reduce costs.
        </span>
        <span class="text-sm text-primary">
          We empower engineers to build the future of technology by making
          advanced build and simulation processes as intuitive and efficient as
          web development. Our commitment is to deliver innovative solutions
          that enable seamless task execution across any environment, ensuring
          that developers can focus on creating transformative technologies that
          drive progress and safety.
        </span>
      </div>
      <picture class="z-10 py-14 w-full md:w-[650px] flex justify-center items-center">
        <img
          src="https://nativelink-cdn.s3.us-east-1.amazonaws.com/tracemachina_logo.webp"
          alt=""
          class="w-3/4"
        />
      </picture>
    </div>
  );
});

const CompanyTeam = component$(() => {
  return (
    <div class="flex px-6 w-full md:w-[1300px] flex-col justify-center items-center gap-5">
      <Label text="Leadership Team" class="w-1/2 md:w-1/5" />
      <div class="flex justify-center items-center flex-col">
        <span class="text-center px-4 text-primary">
          Our team consists of engineers and product leaders from Apple, Google,
          MongoDB, and Toyota that are driven by a desire
        </span>
        <span class="text-center text-primary">
          to build a safer future through next generation simulation
          infrastructure.
        </span>
      </div>

      <div class="relative w-screen px-8 md:w-[1300px] md:justify-evenly md:flex-wrap flex gap-4 snap-x snap-mandatory overflow-x-auto">
        {team.map((member) => (
          <div
            key={member.name}
            class="w-11/12 md:w-1/5 shrink-0 flex flex-col snap-always justify-start items-center snap-center pt-8 pb-12 gap-3"
          >
            <img
              class="rounded-full w-full h-full"
              src={member.img}
              alt={member.name}
            />
            <div class="flex flex-col gap-2">
              <LinearGradient text={member.name} class="text-3xl text-center" />
              <LinearGradient text={member.title} class="text-sm text-center" />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
});

const CompanyContact = component$(() => {
  return (
    <div class="flex px-8 flex-col justify-center items-center gap-5">
      <Label text="Get in Touch" class="text-base w-1/2" />
      <div>
        <LinearGradient
          text="Have questions or need assistance?"
          class="text-base text-center"
        />
        <LinearGradient
          text=" Contact us for general inquiries or sales inquiries."
          class="text-base text-center"
        />
      </div>
    </div>
  );
});

const CompanyCards = component$(() => {
  return (
    <div class="flex px-8 flex-col justify-start md:justify-stretch items-center gap-5 pb-24">
      {_cards.map((card) => (
        <div
          class="shadow-md w-full md:w-full md:flex-row flex flex-col md:justify-between justify-start md:items-center items-start gap-3 rounded-lg py-8 px-10 border border-solid border-primary/40"
          key={card.title}
        >
          <div class="flex flex-col gap-2">
            <h2 class="text-2xl font-thin">{card.title}</h2>
            <p class="text-primary pr-24">{card.description}</p>
          </div>
          <a
            href={card.link}
            class="text-black no-underline! bg-white px-8 py-2 rounded-xl"
            target="_blank"
            rel="noopener noreferrer"
          >
            {card.linkTitle}
          </a>
        </div>
      ))}
    </div>
  );
});

export const CompanyPage = component$(() => {
  useVisibleTask$(() => {
    console.info("About Trace Machina");
  });

  return (
    <main class="flex w-full w-screen flex-col pt-24 items-start justify-start bg-black font-nunito text-white">
      <div class="gap-10 flex w-full flex-col justify-center items-center">
        <CompanyHero />
        <CompanyTeam />
        <CompanyContact />
        <CompanyCards />
      </div>
    </main>
  );
});
