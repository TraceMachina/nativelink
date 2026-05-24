import { component$, useSignal, useVisibleTask$ } from "@builder.io/qwik";
import type { TerminalLine } from "./terminal-data.ts";
import { terminalTabs } from "./terminal-data.ts";

interface DisplayedLine {
  id: number;
  text: string;
}

const lineColorClass = (line: string) => {
  if (line.startsWith("$")) {
    return "text-purple-400 font-semibold";
  }
  if (line.startsWith("Using /")) {
    return "text-blue-400";
  }
  if (line.startsWith("  ")) {
    return "text-gray-400";
  }
  if (
    line.includes("✓") ||
    line.includes("passed") ||
    line.includes("complete") ||
    line.includes("ready")
  ) {
    return "text-green-400";
  }
  if (
    line.includes("Generating") ||
    line.includes("tokens") ||
    line.includes("thought for")
  ) {
    return "text-cyan-300";
  }
  if (line.includes("INFO")) {
    return "text-cyan-400";
  }
  return "text-gray-200";
};

const isShellCommand = (line: TerminalLine) =>
  line.text.startsWith("curl") || line.text.startsWith("docker");

const isCommand = (line: TerminalLine) =>
  isShellCommand(line) || line.text.startsWith(">");

export const AnimatedTerminal = component$(() => {
  const activeTab = useSignal(0);
  const displayedLines = useSignal<DisplayedLine[]>([]);
  const currentInput = useSignal("");
  const isTyping = useSignal(false);
  const terminalRef = useSignal<HTMLDivElement>();

  useVisibleTask$(({ track, cleanup }) => {
    track(() => activeTab.value);

    // Reset animation when tab changes
    displayedLines.value = [];
    currentInput.value = "";
    isTyping.value = false;

    const currentTab = terminalTabs[activeTab.value];
    let timeoutId: number;
    let lineIndex = 0;
    let nextLineId = 0;

    const schedule = (callback: () => void, delay: number) => {
      timeoutId = window.setTimeout(callback, delay);
    };

    const resetLines = () => {
      displayedLines.value = [];
      currentInput.value = "";
      isTyping.value = false;
    };

    const appendLine = (text: string) => {
      displayedLines.value = [
        ...displayedLines.value,
        { id: nextLineId++, text },
      ];
    };

    const replaceLastLine = (text: string) => {
      const nextLines = [...displayedLines.value];
      const lastIndex = nextLines.length - 1;

      if (lastIndex >= 0) {
        const lastLine = nextLines[lastIndex];

        if (lastLine) {
          nextLines[lastIndex] = { ...lastLine, text };
          displayedLines.value = nextLines;
        }
      }
    };

    const lastLineText = () =>
      displayedLines.value[displayedLines.value.length - 1]?.text;

    const scrollToBottom = () => {
      if (terminalRef.value) {
        terminalRef.value.scrollTop = terminalRef.value.scrollHeight;
      }
    };

    const showCommand = (line: TerminalLine) => {
      if (line.instant) {
        const prefix = isShellCommand(line) ? "$ " : "";
        appendLine(prefix + line.text);
        lineIndex++;
        scrollToBottom();
        schedule(animate, line.delay ?? 300);
        return;
      }

      isTyping.value = true;
      currentInput.value = line.text;

      schedule(() => {
        const prefix = isShellCommand(line) ? "$ " : "";
        appendLine(prefix + line.text);
        currentInput.value = "";
        isTyping.value = false;
        lineIndex++;
        scrollToBottom();
        schedule(animate, line.delay ?? 300);
      }, 800);
    };

    const showStatus = (line: TerminalLine) => {
      const statusText = line.text.substring(7);

      if (lastLineText()?.startsWith("Generating..")) {
        replaceLastLine(statusText);
      } else {
        appendLine(statusText);
      }
    };

    const showDownload = (line: TerminalLine) => {
      const [layerId = ""] = line.text.split(":");
      const lastLine = lastLineText();

      if (lastLine?.includes(layerId) && lastLine.includes("Downloading")) {
        replaceLastLine(line.text);
      } else {
        appendLine(line.text);
      }
    };

    const showOutput = (line: TerminalLine) => {
      if (line.text.startsWith("STATUS:")) {
        showStatus(line);
      } else if (line.text.includes("Downloading")) {
        showDownload(line);
      } else {
        appendLine(line.text);
      }

      lineIndex++;
      scrollToBottom();
      const nextDelay = line.instant ? (line.delay ?? 0) : (line.delay ?? 100);
      schedule(animate, nextDelay);
    };

    const animate = () => {
      if (!currentTab) {
        return;
      }

      if (lineIndex >= currentTab.lines.length) {
        // Restart animation after 3 seconds
        schedule(() => {
          resetLines();
          lineIndex = 0;
          animate();
        }, 3000);
        return;
      }

      const line = currentTab.lines[lineIndex];
      if (!line) {
        return;
      }

      if (isCommand(line)) {
        showCommand(line);
      } else {
        showOutput(line);
      }
    };

    animate();

    cleanup(() => {
      if (timeoutId) {
        clearTimeout(timeoutId);
      }
    });
  });

  return (
    <div class="w-full max-w-5xl mx-auto">
      {/* Tab headers */}
      <div class="flex gap-0 mb-0">
        {terminalTabs.map((tab, index) => (
          <button
            key={tab.name}
            type="button"
            onClick$={() => {
              activeTab.value = index;
            }}
            class={`px-4 py-3 font-medium transition-all duration-200 ${
              activeTab.value === index
                ? "bg-[#2d3748] text-white border-t-2 border-l-2 border-r-2 border-[#4a5568] rounded-t"
                : "bg-[#1a202c] text-gray-400 hover:text-gray-300"
            }`}
          >
            {tab.name}
          </button>
        ))}
      </div>

      {/* Terminal window - agentic bottom-input layout */}
      <div class="bg-[#2d3748] rounded-b rounded-tr border-2 border-[#4a5568] h-[500px] flex flex-col overflow-hidden">
        {/* Output area - scrolls */}
        <div
          ref={terminalRef}
          class="flex-1 overflow-y-auto overflow-x-hidden p-6 font-mono text-sm"
        >
          <div class="space-y-1">
            {displayedLines.value.map((line) => (
              <div
                key={line.id}
                class={`break-all overflow-wrap-anywhere ${lineColorClass(line.text)}`}
              >
                {line.text}
              </div>
            ))}
          </div>
        </div>

        {/* Input area - fixed at bottom like Claude Code */}
        <div class="border-t-2 border-[#4a5568] p-4 bg-[#1a202c]">
          <div class="flex items-center gap-3 overflow-hidden">
            <span class="text-purple-400 select-none font-bold text-lg flex-shrink-0">
              {">"}
            </span>
            <div class="flex-1 font-mono text-sm text-white overflow-hidden">
              <span class="break-all">{currentInput.value}</span>
              {isTyping.value && (
                <span class="inline-block w-2 h-4 bg-purple-400 ml-1 animate-pulse flex-shrink-0" />
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
});
