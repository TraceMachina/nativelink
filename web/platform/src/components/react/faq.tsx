/** @jsxImportSource react */

import { motion } from "framer-motion";
import { useEffect, useState } from "react";

const FAQData = [
  {
    question: "What is NativeLink?",
    answer:
      "NativeLink is a high-performance build cache and remote execution system designed to accelerate software compilation and testing while reducing infrastructure costs.",
  },
  {
    question: "How do I set up NativeLink?",
    answer:
      "You can set up NativeLink by deploying it as a Docker image. Detailed setup instructions can be found in the NativeLink documentation.",
  },
  {
    question: "What operating systems are supported by NativeLink?",
    answer:
      "NativeLink supports Unix-based operating systems and Windows, ensuring broad compatibility across different development environments.",
  },
  {
    question: "What are the benefits of using NativeLink?",
    answer:
      "NativeLink significantly reduces build times, especially for incremental changes, by storing and reusing the results of previous build steps. It also distributes build and test tasks across multiple machines for faster completion.",
  },
  {
    question: "Is NativeLink free?",
    answer:
      "Users are free to set up the open-source version of NativeLink, which is limited in functionality but fully open source.",
  },
  {
    question: "What is the Rust-based architecture in NativeLink?",
    answer:
      "NativeLink's Rust-based architecture ensures hermeticity and eliminates garbage collection and race conditions, making it ideal for complex and safety-critical codebases.",
  },
  {
    question: "How does NativeLink handle remote execution?",
    answer:
      "NativeLink distributes build and test tasks across a network of machines, parallelizing workloads and offloading computational burdens from local machines.",
  },
  {
    question: "Can I use NativeLink with my existing build tools?",
    answer:
      "Yes, NativeLink integrates seamlessly with build tools that use the Remote Execution protocol, such as Bazel, Buck2, Goma, and Reclient.",
  },
  {
    question: "How do I ensure my Bazel setup is hermetic with NativeLink?",
    answer:
      "NativeLink ensures a hermetic build environment by eliminating external dependencies and maintaining consistency across builds. Detailed configuration instructions are available in the documentation.",
  },
  {
    question: "Why should I choose NativeLink?",
    answer:
      "NativeLink is trusted by large corporations for its efficiency in reducing costs and developer iteration times. It handles over one billion requests per month and is designed for scalability and reliability.",
  },
];

export const FAQ = () => {
  const [isMounted, setIsMounted] = useState(false);

  useEffect(() => {
    setIsMounted(true);
  }, []);

  const list = (
    <ul className="flex flex-col divide-y divide-[color:var(--color-border-subtle)] rounded-xl border border-[color:var(--color-border-subtle)] bg-[color:var(--color-card)] overflow-hidden">
      {FAQData.map((item) => (
        <li key={item.question}>
          <FAQBox title={item.question} content={item.answer} />
        </li>
      ))}
    </ul>
  );

  return (
    <section className="relative max-w-3xl mx-auto" id="FAQ">
      {isMounted ? (
        <motion.div
          initial={{ opacity: 0 }}
          whileInView={{ opacity: 1 }}
          viewport={{ once: true }}
          transition={{ duration: 0.5, delay: 0.2 }}
        >
          {list}
        </motion.div>
      ) : (
        list
      )}
    </section>
  );
};

interface FAQBoxProps {
  title: string;
  content: string;
}

const FAQBox = ({ title, content }: FAQBoxProps) => {
  const [isOpen, setIsOpen] = useState(false);

  return (
    <button
      type="button"
      aria-expanded={isOpen}
      onClick={() => setIsOpen(!isOpen)}
      className="
            w-full text-left flex flex-col gap-0
            px-5 py-5 sm:px-6
            transition-colors cursor-pointer
            hover:bg-[color:var(--color-card-hover)]
            focus:outline-none focus-visible:bg-[color:var(--color-card-hover)]
         "
    >
      <div className="flex w-full items-center justify-between gap-6">
        <h3 className="text-base sm:text-lg font-medium text-[color:var(--color-fg)] leading-snug">
          {title}
        </h3>
        <svg
          width="20"
          height="20"
          viewBox="0 0 20 20"
          fill="none"
          xmlns="http://www.w3.org/2000/svg"
          className={`shrink-0 transition-transform duration-300 text-[color:var(--color-fg-muted)] ${
            isOpen ? "rotate-180" : "rotate-0"
          }`}
          aria-hidden="true"
        >
          <path
            d="M5 7.5 10 12.5 15 7.5"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </div>
      <div
        className={`grid transition-[grid-template-rows,opacity] duration-300 ease-out ${
          isOpen
            ? "grid-rows-[1fr] opacity-100 mt-3"
            : "grid-rows-[0fr] opacity-0"
        }`}
      >
        <div className="overflow-hidden">
          <p className="text-sm sm:text-base text-[color:var(--color-fg-muted)] leading-relaxed pr-8">
            {content}
          </p>
        </div>
      </div>
    </button>
  );
};
