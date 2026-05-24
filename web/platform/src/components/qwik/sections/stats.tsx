import { component$ } from "@builder.io/qwik";

export const Stats = component$(() => {
  const stats = [
    {
      value: "4-15x",
      label: "faster builds",
      description:
        "Demonstrated on LLVM, one of the world's largest C++ codebases.",
    },
    {
      value: "1B+",
      label: "build requests per month",
      description: "served in production.",
    },
    {
      value: "10 minutes",
      label: "to your first cache hit",
      description: "",
    },
    {
      value: "Zero",
      label: "build-system rewrites",
      description: "required.",
    },
  ];

  return (
    <div class="section-spacing-minor section-divider">
      <div class="max-w-6xl mx-auto px-6">
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-8 md:gap-12">
          {stats.map((stat) => (
            <div key={stat.label} class="flex flex-col gap-2">
              <div class="text-4xl md:text-5xl font-bold text-black">
                {stat.value}
              </div>
              <div class="text-lg font-semibold text-black">{stat.label}</div>
              {stat.description && (
                <div class="text-base text-[rgb(100,100,100)]">
                  {stat.description}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
});
