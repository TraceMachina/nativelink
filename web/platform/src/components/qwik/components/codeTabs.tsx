import { component$, useSignal } from "@builder.io/qwik";

interface CodeTabsProps {
  class: string;
}

export const CodeTabs = component$(
  ({ class: customClass = "" }: CodeTabsProps) => {
    const selectedTab = useSignal("linux");

    return (
      <div
        class={`bg-[#1b1c32] text-white rounded-xl shadow-lg w-full
      ${customClass} `}
      >
        {/* Tabs */}
        <div class="flex gap-1">
          <button
            type="submit"
            onClick$={() => {
              selectedTab.value = "linux";
            }}
            class={`flex-1 m-1 p-3 text-center text-sm! transition-all duration-300 ${
              selectedTab.value === "linux" ? "bg-gray-800" : "bg-gray-700"
            } rounded-tl-xl`}
          >
            Linux x86_64 / Mac OS X
          </button>
          <button
            type="submit"
            onClick$={() => {
              selectedTab.value = "windows";
            }}
            class={`flex-1 m-1 p-4 text-center text-sm! transition-all duration-300 ${
              selectedTab.value === "windows" ? "bg-gray-800" : "bg-gray-700"
            } rounded-tr-xl`}
          >
            Windows x86_64
          </button>
        </div>

        {/* Content */}
        <div class="p-6 h-96 bg-transparent rounded-b-xl overflow-x-auto md:overflow-hidden transition-all duration-300 ">
          {selectedTab.value === "linux" && (
            <pre class="text-sm">
              <code class="whitespace-pre-wrap">
                curl -O \{"\n"}
                https://raw.githubusercontent.com/TraceMachina/nativelink/v0.6.0/nativelink-config/examples/basic_cas.json5
                {"\n\n"}# See{"\n"}
                https://github.com/TraceMachina/nativelink/pkgs/container/nativelink
                {"\n\n"}
                docker run \{"\n"}
                -v $(pwd)/basic_cas.json:/config \{"\n"}
                -p 50051:50051 \{"\n"}
                ghcr.io/tracemachina/nativelink:v0.6.0 \{"\n"}
                config
              </code>
            </pre>
          )}
          {selectedTab.value === "windows" && (
            <pre class="text-sm">
              <code class="whitespace-pre-wrap">
                curl.exe -O \{"\n"}
                https://raw.githubusercontent.com/TraceMachina/nativelink/v0.6.0/nativelink-config/examples/basic_cas.json5
                {"\n\n"}
                docker run \{"\n"}
                -v $(pwd)/basic_cas.json:/config \{"\n"}
                -p 50051:50051 \{"\n"}
                ghcr.io/tracemachina/nativelink:v0.6.0 \{"\n"}
                config
              </code>
            </pre>
          )}
        </div>
      </div>
    );
  },
);
