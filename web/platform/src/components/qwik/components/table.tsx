import { component$ } from "@builder.io/qwik";

import { Checkmark, Xmark } from "./icons.tsx";

const features = [
  {
    feature: "Data Transfer",
    free: "Up to 1TB",
    enterprise: "Unlimited",
  },
  {
    feature: "Hosting",
    free: "Self-hosted",
    enterprise: "Self-hosted or partially to fully managed",
  },
  {
    feature: "Private Hosted Cache",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Support",
    free: "Community Support",
    enterprise: "Dedicated Engineer",
  },
  {
    feature: "Supported Build Systems",
    free: "Bazel, Buck2, Reclient, and Pants",
    enterprise: "Bazel, Buck2, Reclient, and Pants",
  },
  {
    feature: "Operating Systems",
    free: "Linux, MacOS",
    enterprise: "Linux, MacOS, and Windows",
  },
  {
    feature: "Build Action Breakdown",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Live Build Updates",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Org-wide sharing",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "GUI",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Distributed Scheduler",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Autoscaling",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Remote Caching",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Cross-compilation",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "External Storage Cache (S3, Redis)",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Number of CPU cores",
    free: "Up to 100",
    enterprise: "Unlimited",
  },
  {
    feature: "Unlimited users",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Remote Execution",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
];

export const FeatureTable = component$(() => {
  return (
    <table class="table-auto w-full font-thin text-left text-sm md:text-base">
      <thead>
        <tr class="border-b-2 border-[#2b2b2b]">
          <th class="p-2 w-[40%] md:w-[30%] border-r-2 border-[#2b2b2b] font-semibold">
            Feature
          </th>
          <th class="p-2 w-[30%] md:w-[35%] border-r border-[#2b2b2b] text-center font-semibold">
            Free Preview
          </th>
          <th class="p-2 w-[30%] md:w-[35%] text-center font-semibold">
            Enterprise
          </th>
        </tr>
      </thead>
      <tbody>
        {features.map((item) => (
          <tr key={item.feature} class="border-b border-[#2b2b2b]">
            <td class="p-2 border-r-2 border-[#2b2b2b]">{item.feature}</td>
            <td class="p-2 border-r border-[#2b2b2b] text-center">
              {item.free}
            </td>
            <td class="p-2 text-center">{item.enterprise}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
});
