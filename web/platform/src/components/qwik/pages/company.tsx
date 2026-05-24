import { component$ } from "@builder.io/qwik";

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

export const CompanyPage = component$(() => {
  return (
    <main class="bg-[rgb(248,247,244)] min-h-screen">
      {/* Hero Section */}
      <div class="section-spacing-major section-divider">
        <div class="max-w-6xl mx-auto px-6">
          <div class="grid md:grid-cols-2 gap-12 items-center">
            <div class="flex flex-col gap-8">
              <h1 class="text-4xl md:text-6xl font-bold text-black leading-tight">
                NativeLink is built by Trace Machina
              </h1>
              <p class="text-xl text-black leading-relaxed">
                At Trace Machina, our mission is to accelerate the
                reindustrialization of the world in the machine age.
              </p>
              <p class="text-lg text-[rgb(60,60,60)] leading-relaxed">
                We empower engineers to build the future of technology by making
                advanced build and simulation processes that can move at machine
                speed. Our products amplify the rate at which companies can
                innovate across mission critical industries from Semiconductors,
                Advanced Robotics, Autonomous Vehicles, Artificial Intelligence
                Research to Life Sciences and Financial Services. Our commitment
                is to amplify execution across any environment, ensuring that
                developers can focus on creating transformative technologies
                that drive forward progress on behalf of humanity.
              </p>
            </div>
            <div class="flex justify-center items-center">
              <img
                src="https://nativelink-cdn.s3.us-east-1.amazonaws.com/tracemachina_logo.webp"
                alt="Trace Machina Logo"
                class="w-full max-w-md"
              />
            </div>
          </div>
        </div>
      </div>

      {/* Contact Section */}
      <div class="section-spacing-major">
        <div class="max-w-4xl mx-auto px-6">
          <div class="text-center mb-12">
            <p class="text-sm uppercase tracking-wide text-[rgb(100,100,100)] font-semibold mb-4">
              Get in Touch
            </p>
            <h2 class="text-3xl md:text-5xl font-bold text-black mb-4">
              Have questions or need assistance?
            </h2>
            <p class="text-xl text-[rgb(60,60,60)]">
              Contact us for general inquiries or sales inquiries.
            </p>
          </div>

          {/* Contact Cards */}
          <div class="flex flex-col gap-6">
            {_cards.map((card) => (
              <div
                key={card.title}
                class="card-warm p-8 flex flex-col md:flex-row md:justify-between md:items-center gap-6"
              >
                <div class="flex flex-col gap-3 flex-1">
                  <h3 class="text-2xl font-bold text-black">{card.title}</h3>
                  <p class="text-lg text-[rgb(60,60,60)] leading-relaxed">
                    {card.description}
                  </p>
                </div>
                <a
                  href={card.link}
                  class="inline-flex bg-black text-white hover:bg-[rgb(40,40,40)] transition-colors duration-200 px-8 min-h-[48px] rounded-interactive justify-center items-center border-2 border-black font-semibold whitespace-nowrap no-underline"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {card.linkTitle}
                </a>
              </div>
            ))}
          </div>
        </div>
      </div>
    </main>
  );
});
