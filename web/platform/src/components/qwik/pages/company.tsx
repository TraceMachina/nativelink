import { component$, useVisibleTask$ } from "@builder.io/qwik";

import { Label, LinearGradient } from "../components/text";

import { BackgroundVideo } from "../components/video";

const team = [
  {
    img: "https://static.wixstatic.com/media/b37b63_0656d59454a2475ebbd617004a6ca8cb~mv2.jpg/v1/fill/w_618,h_618,al_c,q_85,usm_0.66_1.00_0.01,enc_auto/Andrew%20Shipley%20-%20Director%20of%20Operations_edited.jpg",
    name: "Andrew Shipley",
    title: "Director of Operations",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_06a9f6629b70404895da49decb2954a0~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Aaron%20Mondal%20-%20Software%20Engineer_edited.jpg",
    name: "Aaron Mondal",
    title: "Software Engineer",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_14329a284c0f4a7c9abae9d9171b178b~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Tim%20Potter%20-%20Chief%20Technology%20Officer_edited.jpg",
    name: "Tim Potter",
    title: "Chief Technology Officer",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_9c67683dba7a422fbf1cb197daa920b9~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Brian%20Clark%20-%20Software%20Engineer_edited.jpg",
    name: "Brian Clark",
    title: "Software Engineer",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_afa6ff90e65944c4a0a241360ffd4b50~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Tyrone%20Greenfield%20_%20Chief%20of%20Staff_edited.jpg",
    name: "Tyrone Greenfield",
    title: "Chief of Staff",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_8f270884e7b945caaf1a6293265386f5~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Nadine%20Farah%20-%20Product%20Marketing_edited.jpg",
    name: "Nadine Farah",
    title: "Product Marketing",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_30b07f1bbce4444d8ab9fb6e4902db6a~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/AdamSinger_edited.jpg",
    name: "Adam Singer",
    title: "Software Engineer",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_c4d658064531477489510fae5dcca594~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Blake%20Hatch%20-%20Developer%20Relations_Cloud%20Engineering_edited.jpg",
    name: "Blake Hatch",
    title: "Business Engineer",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_f5492bab76244be9a590f0b7129fd2af~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Marcus%20Eagan%20-%20Co-Founder_edited.jpg",
    name: "Marcus Eagan",
    title: "Co-Founder",
  },
  {
    img: "https://static.wixstatic.com/media/b37b63_40a02174d13c4f2fa6a1894ad2505499~mv2.jpg/v1/fill/w_618,h_618,al_c,q_80,usm_0.66_1.00_0.01,blur_2,enc_auto/Blaise%20Bruer%20-%20Co-Founder_edited.jpg",
    name: "Blaise Bruer",
    title: "Co-Founder",
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
          src="https://static.wixstatic.com/media/e16379_35198dafb70c4384935c816f2f5d47e7~mv2.png/v1/fill/w_542,h_518,fp_0.50_0.50,lg_1,q_85,enc_auto/TM-logo.png"
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
      <Label text="meet our team" class="w-1/2 md:w-1/5" />
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
