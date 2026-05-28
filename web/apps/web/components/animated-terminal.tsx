"use client";

import { useEffect, useRef, useState } from "react";
import type { TerminalLine } from "./terminal-data";
import { terminalTabs } from "./terminal-data";

interface DisplayedLine {
  id: number;
  text: string;
}

const cx = (...classes: Array<string | false | null | undefined>) =>
  classes.filter(Boolean).join(" ");

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
  if (line.includes("Generating") || line.includes("tokens") || line.includes("thought for")) {
    return "text-cyan-300";
  }
  if (line.includes("INFO")) {
    return "text-cyan-400";
  }
  return "text-gray-200";
};

const isShellCommand = (line: TerminalLine) =>
  line.text.startsWith("curl") || line.text.startsWith("docker");

const isCommand = (line: TerminalLine) => isShellCommand(line) || line.text.startsWith(">");

export function AnimatedTerminal() {
  const [activeTab, setActiveTab] = useState(0);
  const [displayedLines, setDisplayedLines] = useState<DisplayedLine[]>([]);
  const [currentInput, setCurrentInput] = useState("");
  const [isTyping, setIsTyping] = useState(false);
  const terminalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setDisplayedLines([]);
    setCurrentInput("");
    setIsTyping(false);

    const currentTab = terminalTabs[activeTab];
    if (!currentTab) {
      return;
    }

    const timeoutIds: number[] = [];
    let isCancelled = false;
    let lineIndex = 0;
    let nextLineId = 0;
    let linesSnapshot: DisplayedLine[] = [];

    const schedule = (callback: () => void, delay: number) => {
      const timeoutId = window.setTimeout(() => {
        if (!isCancelled) {
          callback();
        }
      }, delay);
      timeoutIds.push(timeoutId);
    };

    const scrollToBottom = () => {
      if (isCancelled) {
        return;
      }
      if (terminalRef.current) {
        terminalRef.current.scrollTop = terminalRef.current.scrollHeight;
      }
    };

    const appendLine = (text: string) => {
      if (isCancelled) {
        return;
      }
      linesSnapshot = [...linesSnapshot, { id: nextLineId++, text }];
      setDisplayedLines(linesSnapshot);
    };

    const replaceLastLine = (text: string) => {
      if (isCancelled) {
        return;
      }
      const nextLines = [...linesSnapshot];
      const lastIndex = nextLines.length - 1;

      if (lastIndex >= 0) {
        const lastLine = nextLines[lastIndex];
        if (lastLine) {
          nextLines[lastIndex] = { ...lastLine, text };
          linesSnapshot = nextLines;
          setDisplayedLines(linesSnapshot);
        }
      }
    };

    const lastLineText = () => linesSnapshot[linesSnapshot.length - 1]?.text;

    const resetLines = () => {
      if (isCancelled) {
        return;
      }
      linesSnapshot = [];
      setDisplayedLines([]);
      setCurrentInput("");
      setIsTyping(false);
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

    const animate = () => {
      if (isCancelled) {
        return;
      }
      if (lineIndex >= currentTab.lines.length) {
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
        if (line.instant) {
          const prefix = isShellCommand(line) ? "$ " : "";
          appendLine(prefix + line.text);
          lineIndex++;
          scrollToBottom();
          schedule(animate, line.delay ?? 300);
          return;
        }

        setIsTyping(true);
        setCurrentInput(line.text);

        schedule(() => {
          if (isCancelled) {
            return;
          }
          const prefix = isShellCommand(line) ? "$ " : "";
          appendLine(prefix + line.text);
          setCurrentInput("");
          setIsTyping(false);
          lineIndex++;
          scrollToBottom();
          schedule(animate, line.delay ?? 300);
        }, 800);
        return;
      }

      if (line.text.startsWith("STATUS:")) {
        showStatus(line);
      } else if (line.text.includes("Downloading")) {
        showDownload(line);
      } else {
        appendLine(line.text);
      }

      lineIndex++;
      scrollToBottom();
      schedule(animate, line.instant ? (line.delay ?? 0) : (line.delay ?? 100));
    };

    animate();

    return () => {
      isCancelled = true;
      for (const timeoutId of timeoutIds) {
        window.clearTimeout(timeoutId);
      }
    };
  }, [activeTab]);

  return (
    <div className="mx-auto w-full max-w-5xl">
      <div className="mb-0 flex gap-0 overflow-x-auto">
        {terminalTabs.map((tab, index) => (
          <button
            key={tab.name}
            type="button"
            onClick={() => setActiveTab(index)}
            className={cx(
              "min-h-11 shrink-0 px-3 py-3 font-medium text-sm transition-colors duration-200 md:px-4",
              activeTab === index
                ? "rounded-t border-t-2 border-r-2 border-l-2 border-[#4a5568] bg-[#2d3748] text-white"
                : "bg-[#1a202c] text-gray-400 hover:text-gray-300",
            )}
          >
            {tab.name}
          </button>
        ))}
      </div>

      <div className="flex h-[500px] flex-col overflow-hidden rounded-r rounded-b border-2 border-[#4a5568] bg-[#2d3748]">
        <div
          ref={terminalRef}
          className="flex-1 overflow-y-auto overflow-x-hidden p-6 font-mono text-sm"
        >
          <div className="space-y-1">
            {displayedLines.map((line) => (
              <div key={line.id} className={cx("break-all", lineColorClass(line.text))}>
                {line.text}
              </div>
            ))}
          </div>
        </div>

        <div className="border-t-2 border-[#4a5568] bg-[#1a202c] p-4">
          <div className="flex items-center gap-3 overflow-hidden">
            <span className="shrink-0 select-none font-bold text-lg text-purple-400">{">"}</span>
            <div className="flex-1 overflow-hidden font-mono text-sm text-white">
              <span className="break-all">{currentInput}</span>
              {isTyping && (
                <span className="ml-1 inline-block h-4 w-2 shrink-0 animate-pulse bg-purple-400" />
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
