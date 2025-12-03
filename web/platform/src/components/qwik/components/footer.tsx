import { $, component$, useSignal } from "@builder.io/qwik";
import { BackgroundVideo } from "./video.tsx";

const Logo =
  "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_logo.webp";

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
    <footer class="relative w-full text-white bg-black py-4 overflow-hidden">
      <div class="absolute bottom-0 mx-0 left-0 right-0 w-full h-full ">
        {/* Background Video */}
        <BackgroundVideo class="absolute bottom-[0] md:bottom-[-20vh] left-0 z-0 h-full md:h-auto object-cover md:w-screen" />
        {/* <div class="absolute w-[150vw] bottom-0 left-1/2 transform -translate-x-1/2 translate-y-5/12">
            <Background class="rotate-180 w-full" />
        </div> */}

        <div class="absolute bottom-0 justify-center items-center overflow-hidden">
          <div class="bg-black/50 w-screen h-screen" />
        </div>
        {/* <div class="absolute flex justify-center items-center overflow-hidden">
          <div class="bg-black/50 w-screen h-screen" />
        </div> */}
        {/* Overlay Image */}
        {/* <img
          src={Overlay.src}
          class="absolute inset-0 h-full w-full object-cover"
          alt="Overlay"
        /> */}
      </div>

      <div class="relative z-10 flex h-full w-full flex-col items-center justify-center gap-6 md:flex-row md:gap-0">
        <div class="flex w-full items-center justify-center text-[2.5rem] leading-none tracking-normal md:text-[52px] ">
          <div class="w-full  flex flex-col items-center">
            <div class="text-center">
              Lets <span class="text-[#AD96FF]">build</span> together.
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
                <label class="mb-2 block text-lg font-thin" for="email">
                  Sign up for our newsletter *
                </label>
                <input
                  class="flex focus:shadow-outline w-full bg-white appearance-none rounded-2xl border px-3 py-3 leading-tight text-gray-700 shadow focus:outline-none"
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
                    class={`w-full text-center ${
                      message.value.toLowerCase().includes("failed")
                        ? "text-red-500"
                        : "text-green-500"
                    }`}
                  >
                    {message.value}
                  </div>
                )}
              </div>
              <div class="w-full flex items-center justify-between">
                <button
                  class="focus:shadow-outline w-full rounded-full bg-purple-600 px-4 py-3 font-thin text-white hover:bg-purple-700 focus:outline-none"
                  type="button"
                  onClick$={handleSubmit}
                >
                  Subscribe
                </button>
              </div>
            </form>

            <div
              id="footer-links"
              class="w-full flex flex-col justiy-center gap-2 items-start font-thin"
            >
              {_links.map((link, _index) => (
                <a key={link.name} href={link.link}>
                  {link.name}
                </a>
              ))}
            </div>

            <div
              id="footer-links"
              class="w-full flex flex-col justiy-center gap-2 items-start font-thin"
            >
              {law.map((link, _index) => (
                <a key={link.name} href={link.link}>
                  {link.name}
                </a>
              ))}
            </div>

            {/* <div class="text-[22px]">
              <span class="text-[#AD96FF]">Get in touch:</span>{" "}
              <a href="mailto:hello@nativelink.com">hello@nativelink.com</a>
            </div> */}

            <div class="flex flex-col">
              <a href="/" class="z-50">
                <img
                  src={Logo}
                  loading="lazy"
                  class="w-5/12"
                  alt="Nativelink Logo"
                />
              </a>
              <span class="text-white! font-thin">© Trace Machina 2025</span>
            </div>
          </div>
        </div>
      </div>
    </footer>
  );
});
