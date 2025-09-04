/** @jsxImportSource react */
import { useEffect, useState } from "react";

import { motion } from "framer-motion";

const FAQData = [
  {
    question: "What is NativeLink?",
    answer:
      "NativeLink is a high-performance build cache and remote execution system designed to accelerate software compilation and testing while reducing infrastructure costs.",
  },
  {
    question: "How do I set up NativeLink?",
    answer:
      "You can set up NativeLink by deploying it as a Docker image or using the cloud-hosted solution. Detailed setup instructions can be found in the NativeLink documentation.",
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
      "No, NativeLink is now a paid service. Our Starter plan begins at $29/month. Users are free to set up the open-source version of NativeLink, which is limited in functionality but fully open source.",
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

  return (
    <section className="relative -mt-8 sm:mt-0 pt-12 sm:pt-16 bg-blueGray-50 overflow-hidden">
      <div className="absolute -top-10" id="FAQ" />
      {isMounted ? (
        <motion.div
          initial={{ opacity: 0 }}
          whileInView={{ opacity: 1 }}
          viewport={{ once: true }}
          transition={{ duration: 0.5, delay: 0.2 }}
        >
          <div className="relative z-10 container mx-auto sm:w-full">
            <div className="md:max-w-4xl mx-auto">
              <div className="mb-8 flex flex-wrap -m-1">
                {FAQData.map((item, index) => (
                  <div className="w-full p-1" key={`${item.question}-${index}`}>
                    <FAQBox title={item.question} content={item.answer} />
                  </div>
                ))}
              </div>
            </div>
          </div>
        </motion.div>
      ) : (
        <div className="relative z-10 container mx-auto sm:w-full">
          <div className="md:max-w-4xl mx-auto">
            <div className="mb-8 flex flex-wrap -m-1">
              {FAQData.map((item, index) => (
                <div className="w-full p-1" key={`${item.question}-${index}`}>
                  <FAQBox title={item.question} content={item.answer} />
                </div>
              ))}
            </div>
          </div>
        </div>
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
      className="w-full text-left rounded-3xl flex justify-between flex-row items-center hover:bg-bgDark3Hover cursor-pointer transition"
      onClick={() => setIsOpen(!isOpen)}
    >
      <div
        className={
          "flex flex-col justify-center items-start border-b border-[#2b2b2b] w-full"
        }
      >
        <div className="w-full flex flex-row justify-between py-4 border-b border-[#2b2b2b]">
          <h3 className="content-title pt-3 sm:pt-0 pr-8 sm:pr-0 bg-gradient-to-r from-white to-[#707098] bg-clip-text leading-none tracking-normal text-transparent">
            {title}
          </h3>
          <svg
            width="28px"
            height="30px"
            viewBox="0 0 20 20"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            className={`transition-all duration-500 ${
              isOpen ? "rotate-[180deg]" : "rotate-[270deg]"
            }`}
          >
            <title>Toggle FAQ</title>
            <path
              d="M4.16732 12.5L10.0007 6.66667L15.834 12.5"
              stroke="#8280a6"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </div>
        <p
          className={`text-secondaryText transition-all duration-400 overflow-hidden ${
            isOpen
              ? "max-h-96 opacity-100 text-white py-4"
              : "max-h-0 opacity-0"
          }`}
        >
          {content}
        </p>
      </div>
    </button>
  );
};
