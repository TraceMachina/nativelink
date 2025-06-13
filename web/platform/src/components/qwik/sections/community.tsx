import { component$ } from "@builder.io/qwik";

import { LinearGradient } from "../components/text.tsx";

import { GitHub, Slack } from "../../media/icons/icons.tsx";

const _docsIcon =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/docs_icon.webp";

const communityLinks = [
  {
    name: "Docs",
    icon: _docsIcon,
    link: "/docs/introduction/setup",
    ariaLabel: "NativeLink documentation",
  },
  {
    name: "Slack",
    icon: <Slack />,
    link: "https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A",
    ariaLabel: "NativeLink Slack channel",
  },
  {
    name: "GitHub",
    icon: <GitHub />,
    link: "https://github.com/tracemachina/nativelink",
    ariaLabel: "NativeLink GitHub repository",
  },
];

export const Community = component$(() => {
  return (
    <div class="pb-16 flex flex-col md:flex-row gap-10 md:justify-evenly md:py-24 md:pb-46">
      <LinearGradient
        text="Join our community"
        class="flex justify-center items-center text-5xl md:text-5xl"
      />
      <div class="flex flex-col md:flex-row justify-start md:justify-evenly items-center gap-10">
        {communityLinks.map((communityLink) => (
          <a
            key={communityLink.name}
            href={communityLink.link}
            aria-label={communityLink.ariaLabel}
            class="hover:-translate-y-1 no-underline! hover:border-[purple] transition-all duration-200 bg-white/6 w-64 h-20 flex justify-center items-center rounded-[16px] shadow-md shadow-black/10 backdrop-blur-[5px] border border-white/30"
          >
            <div class="flex items-center gap-2">
              {typeof communityLink.icon === "string" ? (
                <img src={communityLink.icon} alt="" class="w-12 h-12" />
              ) : (
                communityLink.icon
              )}
              {communityLink.name === "Docs" && <div>{communityLink.name}</div>}
            </div>
          </a>
        ))}
      </div>
    </div>
  );
});
