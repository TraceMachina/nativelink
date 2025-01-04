import { component$, useVisibleTask$ } from "@builder.io/qwik";

import { GitHub, Slack } from "../../media/icons/icons";

import { Label, LinearGradient } from "../components/text";

const connectOn = [
  {
    title: "Connect on Slack",
    description:
      "Join our Slack community to connect with fellow NativeLink users, share tips, and get support from our community and team.",
    link: "https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A",
    icon: <Slack />,
  },
  {
    title: "Collaborate on GitHub",
    description:
      "Explore our open-source projects, contribute to the codebase, and collaborate with other developers.",
    link: "https://github.com/tracemachina/nativelink",
    icon: <GitHub />,
  },
];

const _events = [
  {
    title: "Bazelcon",
    date: "October 14-15, 2024",
    description:
      "Come hang out with us at Bazelcon at the Computer History Museum in Mountain View, CA! We are speaking at the event, so drop by to hear about the cool stuff we're building with our open-source contributors and the impact we've had for our customers. Also come find us around the conference if you want to chat with our team, we love to talk shop and meet our community in-person. Keep an eye on our socials for info on our external meetups that week and other chances to connect with our team during the conference. 1401 N Shoreline Blvd, Mountain View, CA 94043. Hope to see you there!",
  },
  {
    title: "NativeLink's Pre-launch Event",
    date: "August 13th, 2024",
    description:
      "🎉 Get ready to celebrate with NativeLink Join us on August 13, 2024, from 6:30 PM to 9:30 PM at a private cozy location in San Francisco. NativeLink is excited to provide an evening of fun, excitement, and a sneak peek into what we have been cooking up. Expect good food, great company, and vibes! We can't wait to see you there! 🎈✨",
  },
  {
    title: "NativeLink's Pre-launch Party NYC",
    date: "August 9th, 2024",
    description:
      "Join us for an open bar event at the stunning Loft in Flatiron, Manhattan to celebrate NativeLink coming out of stealth! NativeLink is a high-performance build cache and remote execution server, compatible with Bazel, Buck2, Reclient, and other RBE-compatible build systems. It offers drastically faster software builds and hardware test simulations, while reducing infrastructure costs and improving developer experience. Be sure to ⭐️ our project to show your support and get off the waitlist: https://github.com/TraceMachina/nativelink Loft in Flatiron 20 W 23rd St Suite 4, New York, NY 10010, USA",
  },
];

export const CommunityPage = component$(() => {
  useVisibleTask$(() => {
    // console.info("You can find more about the NativeLink Products here.");
  });

  return (
    <main class="pt-36 flex gap-10 bg-black justify-center flex-col items-center px-8 w-screen text-white text-4xl">
      <div class="relative flex flex-col w-full md:w-[1300px] gap-10 items-start justify-center h-96">
        {/* Backgrounds */}
        <div class="z-0 absolute inset-0">
          <div class="w-96 h-96 bg-gradient-to-b from-blue-400 to-purple-500 rounded-full blur-3xl opacity-50 absolute -top-40 -left-60 md:top-24 md:-left-96" />
        </div>
        <div class="z-0 absolute inset-0">
          <div class="w-36 h-36 bg-purple-500 rounded-full blur-3xl opacity-50 absolute top-64 right-20 md:-right-80" />
        </div>

        <LinearGradient
          text={"Join our community"}
          class="text-4xl md:text-5xl text-left z-10"
        />
        <div class="flex justify-start items-strt flex-col md:flex-row gap-12 z-10">
          {connectOn.map(({ title, description, link, icon }) => (
            <div key={title} class="flex flex-col gap-5 pb-6">
              <LinearGradient text={title} class="text-lg text-left" />
              <span class="text-sm">{description}</span>
              <a href={link}>{icon}</a>
            </div>
          ))}
        </div>
      </div>

      <div class="flex justify-start py-12 items-start md:items-center flex-col gap-10 md:w-[1300px]">
        <LinearGradient
          text={"Participate in NativeLink Events"}
          class="text-2xl md:text-3xl text-left md:text-center"
        />

        <span class="text-base flex flex-col justify-start items-start md:text-center">
          Stay updated on upcoming events, webinars, and meetups.
          <br />
          Learn from experts and network with peers.
        </span>
        <div class="flex justify-start items-strt md:flex-row md:w-full flex-col gap-10">
          {_events.map(({ title, date, description }) => (
            <div
              key={title}
              class="flex flex-col gap-5 border-[rgb(57,64,75)] md:w-1/3 px-5 py-8 border rounded-lg"
            >
              <LinearGradient text={title} class="text-lg text-left " />
              <span class="text-sm">{date}</span>
              <span class="text-sm text-primary">{description}</span>
            </div>
          ))}
        </div>
      </div>

      <div class="w-full flex justify-center items-center flex-col gap-10 md:w-[1300px] md:pb-16">
        {/* <LinearGradient
          text={"We Love Our Community"}
          size="2xl"
          position="left"
        /> */}
        <h2 class="text-2xl">We Love Our Community</h2>
        <span class="text-base text-center">
          To say thank you to our NativeLink contributors and community members,
          we’d love to send you something special as a small token of
          appreciation. Simply fill out the form below, our team will verify
          your details, and send something special your way.
        </span>
        <Label text="Fill out the form" class="text-sm" />
      </div>
    </main>
  );
});
