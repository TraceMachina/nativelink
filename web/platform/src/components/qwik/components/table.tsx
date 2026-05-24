import { component$ } from "@builder.io/qwik";

import { Checkmark, Xmark } from "./icons.tsx";

const features = [
  {
    feature: "Hosting",
    free: "Self-hosted",
    enterprise: "Self-hosted or partially managed",
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
    feature: "Org-wide sharing",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Distributed Scheduler",
    free: <Checkmark />,
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
    feature: "Remote Execution",
    free: <Checkmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Autoscaling",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "GUI",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Build Action Breakdown",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
  {
    feature: "Live Build Updates",
    free: <Xmark />,
    enterprise: <Checkmark />,
  },
];

export const FeatureTable = component$(() => {
  return (
    <div class="card-warm overflow-hidden">
      <table class="table-auto w-full text-left text-base">
        <thead>
          <tr class="border-b-2 border-[rgb(220,220,220)] bg-white/50">
            <th class="p-4 w-[40%] md:w-[30%] border-r-2 border-[rgb(220,220,220)] font-bold text-black">
              Feature
            </th>
            <th class="p-4 w-[30%] md:w-[35%] border-r-2 border-[rgb(220,220,220)] text-center font-bold text-black">
              Open Source
            </th>
            <th class="p-4 w-[30%] md:w-[35%] text-center font-bold text-black">
              Enterprise
            </th>
          </tr>
        </thead>
        <tbody>
          {features.map((item) => (
            <tr key={item.feature} class="border-b border-[rgb(220,220,220)]">
              <td class="p-4 border-r-2 border-[rgb(220,220,220)] text-black font-semibold">
                {item.feature}
              </td>
              <td class="p-4 border-r-2 border-[rgb(220,220,220)] text-center text-[rgb(60,60,60)]">
                {item.free}
              </td>
              <td class="p-4 text-center text-[rgb(60,60,60)]">
                {item.enterprise}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
});
