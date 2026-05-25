import { $, component$, useSignal } from "@builder.io/qwik";

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
    <footer class="w-full text-black border-t border-[rgb(210,210,210)] bg-[rgb(248,247,244)]">
      <div class="max-w-6xl mx-auto px-6 md:px-10">
        {/* ── CTA strip ── */}
        <div class="py-14 md:py-16 border-b border-[rgb(220,220,220)]">
          <div class="max-w-2xl">
            <h2 class="text-2xl md:text-[2rem] font-bold leading-snug tracking-tight mb-2">
              Let's build at the speed your code is being written.
            </h2>
            <p class="text-sm md:text-base text-[rgb(80,80,80)]">
              Open source. Free cloud tier. Self-host when you&apos;re ready.
            </p>
          </div>
        </div>

        {/* ── Links + Newsletter grid ── */}
        <div class="py-12 grid grid-cols-1 md:grid-cols-3 gap-10 border-b border-[rgb(220,220,220)]">
          {/* Nav links */}
          <div class="flex flex-col gap-2.5">
            <p class="text-xs font-semibold uppercase tracking-widest text-[rgb(120,120,120)] mb-1">
              Product
            </p>
            {_links.map((link) => (
              <a
                key={link.name}
                href={link.link}
                class="text-sm text-[rgb(50,50,50)] hover:text-black transition-colors duration-150 no-underline"
              >
                {link.name}
              </a>
            ))}
          </div>

          {/* Legal links */}
          <div class="flex flex-col gap-2.5">
            <p class="text-xs font-semibold uppercase tracking-widest text-[rgb(120,120,120)] mb-1">
              Company
            </p>
            {law.map((link) => (
              <a
                key={link.name}
                href={link.link}
                class="text-sm text-[rgb(50,50,50)] hover:text-black transition-colors duration-150 no-underline"
              >
                {link.name}
              </a>
            ))}
          </div>

          {/* Newsletter */}
          <div class="flex flex-col gap-3">
            <p class="text-xs font-semibold uppercase tracking-widest text-[rgb(120,120,120)] mb-1">
              Newsletter
            </p>
            <form
              class="flex flex-col gap-2.5"
              onSubmit$={(e) => e.preventDefault()}
            >
              <label class="text-sm text-[rgb(50,50,50)]" for="footer-email">
                Stay up to date with NativeLink.
              </label>
              <input
                class="w-full bg-white border border-[rgb(210,210,210)] rounded-lg px-3 py-2.5 text-sm text-black placeholder-[rgb(160,160,160)] focus:outline-none focus:border-[rgb(100,100,100)] transition-colors"
                id="footer-email"
                name="data.email"
                type="email"
                placeholder="you@example.com"
                bind:value={email}
                required={true}
                autocomplete="email"
              />
              {message.value && (
                <p
                  class={`text-xs ${
                    message.value.toLowerCase().includes("failed")
                      ? "text-red-600"
                      : "text-green-700"
                  }`}
                >
                  {message.value}
                </p>
              )}
              <button
                class="w-full rounded-lg bg-black text-white text-sm font-medium py-2.5 px-4 hover:bg-[rgb(30,30,30)] focus:outline-none transition-colors duration-200"
                type="button"
                onClick$={handleSubmit}
              >
                Subscribe
              </button>
            </form>
          </div>
        </div>

        {/* ── Bottom bar ── */}
        <div class="py-6 flex flex-col md:flex-row items-start md:items-center justify-between gap-4">
          <a href="/" class="shrink-0">
            <img src={Logo} loading="lazy" class="w-28" alt="Nativelink Logo" />
          </a>
          <span class="text-xs text-[rgb(120,120,120)]">
            © Trace Machina 2026
          </span>
        </div>
      </div>
    </footer>
  );
});
