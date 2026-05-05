import { $, component$, useSignal } from "@builder.io/qwik";
import { BackgroundVideo } from "./video.tsx";

const Logo = "/logo-light.svg";

const _links = [
  {
    name: "Community",
    link: "/community",
  },
  {
    name: "Product",
    link: "/product",
  },
  {
    name: "Documentation",
    link: "/docs/introduction/setup",
  },
  {
    name: "Company",
    link: "/company",
  },
];

const law = [
  {
    name: "Terms and Privacy",
    link: "/terms",
  },
  {
    name: "Compliance",
    link: "/compliance",
  },
  {
    name: "Contact Us",
    link: "mailto:contact@nativelink.com",
  },
  {
    name: "Resources",
    link: "/resources",
  },
];

export const Footer = component$(() => {
  const email = useSignal("");
  const message = useSignal("");

  const handleSubmit = $(async () => {
    try {
      const response = await fetch("/api/mail.json", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          email: email.value,
        }),
      });

      const data = await response.json();
      if (response.ok) {
        email.value = "";
        message.value = data.message;
        console.info("Email successfully sent!");
      } else {
        message.value = data.error || "Failed to subscribe. Try again later.";
        console.error("Failed to send email.");
      }
    } catch (error) {
      message.value = "An unexpected error occurred. Please try again later.";
      console.error("Error sending email:", error);
    }
  });

  return (
    <footer class="relative w-full text-black border-t border-[rgb(180,180,180)] py-16 md:py-24 bg-[rgb(248,247,244)]">
      <div class="relative z-10 flex h-full w-full flex-col items-center justify-center gap-12 md:flex-row md:gap-0 px-6">
        <div class="flex w-full items-center justify-center text-[2.5rem] leading-none tracking-normal md:text-[52px]">
          <div class="w-full flex flex-col items-center">
            <div class="text-center">
              Build infrastructure for the agentic era
            </div>
          </div>
        </div>

        <div class="flex w-full items-center justify-center ">
          <div class="flex w-3/4 flex-col items-start gap-10">
            <form
              class="py-5 flex w-full max-w-md flex-col items-start justify-center gap-4 md:flex-row md:items-end md:justify-start"
              onSubmit$={(e) => e.preventDefault()}
            >
              <div class="w-full">
                <label class="mb-2 block text-base font-normal" for="email">
                  Sign up for our newsletter *
                </label>
                <input
                  class="flex w-full bg-white appearance-none rounded-interactive border-2 border-[rgb(220,220,220)] px-4 py-3 leading-tight text-black focus:outline-none focus:border-black"
                  id="email"
                  name="data.email"
                  type="email"
                  placeholder="Enter email address"
                  bind:value={email}
                  required={true}
                  autocomplete="email"
                />
                {message.value && (
                  <div
                    class={`w-full text-center mt-2 text-sm ${
                      message.value.toLowerCase().includes("failed")
                        ? "text-red-600"
                        : "text-green-600"
                    }`}
                  >
                    {message.value}
                  </div>
                )}
              </div>
              <div class="w-full flex items-center justify-between">
                <button
                  class="w-full rounded-interactive bg-black px-6 py-3 min-h-[48px] font-normal text-white hover:bg-[rgb(40,40,40)] focus:outline-none transition-colors duration-200"
                  type="button"
                  onClick$={handleSubmit}
                >
                  Subscribe
                </button>
              </div>
            </form>

            <div
              id="footer-links"
              class="w-full flex flex-col justiy-center gap-3 items-start font-normal"
            >
              {_links.map((link, _index) => (
                <a key={link.name} href={link.link} class="hover:opacity-60 transition-opacity">
                  {link.name}
                </a>
              ))}
            </div>

            <div
              id="footer-links"
              class="w-full flex flex-col justiy-center gap-3 items-start font-normal"
            >
              {law.map((link, _index) => (
                <a key={link.name} href={link.link} class="hover:opacity-60 transition-opacity">
                  {link.name}
                </a>
              ))}
            </div>

            <div class="flex flex-col gap-2">
              <a href="/" class="z-50">
                <img
                  src={Logo}
                  loading="lazy"
                  class="w-32"
                  alt="Nativelink Logo"
                />
              </a>
              <span class="text-black font-normal">© Trace Machina 2025</span>
            </div>
          </div>
        </div>
      </div>
    </footer>
  );
});
