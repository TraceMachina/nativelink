"use client";

import { Button } from "@nativelink/ui";
import { type FormEvent, useState } from "react";
import { emailFormatError } from "../lib/email";

/**
 * Asks for an email and emails the team, who send the Slack invite by hand.
 * Replaces the Google Form that used to sit behind every "Join Slack" link.
 */
export function SlackJoinForm() {
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [isSuccess, setIsSuccess] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setMessage("");
    setIsSuccess(false);

    const address = email.trim();
    const invalid = emailFormatError(address);
    if (invalid) {
      setMessage(invalid);
      return;
    }

    setIsLoading(true);
    try {
      const response = await fetch("/api/slack-invite", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: address }),
      });
      const data = (await response.json().catch(() => ({}))) as { error?: string };
      if (!response.ok) {
        throw new Error(data.error || "Failed to send your request");
      }
      setIsSuccess(true);
      setMessage(`Thanks. We'll send an invite to ${address}, usually within a day.`);
      setEmail("");
    } catch (error) {
      setIsSuccess(false);
      setMessage(
        error instanceof Error && error.message
          ? error.message
          : "An unexpected error occurred. Please try again later.",
      );
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <div className="rounded-2xl border border-border bg-surface p-8 shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.35)] md:p-10">
      <form onSubmit={handleSubmit} className="space-y-4">
        <div>
          <label
            className="mb-2 block font-mono text-[11px] uppercase tracking-[0.14em] text-muted"
            htmlFor="slack-email"
          >
            Email address
          </label>
          <input
            id="slack-email"
            name="email"
            type="email"
            placeholder="you@example.com"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            required
            autoComplete="email"
            disabled={isLoading || isSuccess}
            className="w-full rounded-lg border border-border bg-background px-4 py-3 text-[15px] text-foreground placeholder:text-muted transition-colors focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/40 disabled:opacity-50"
          />
        </div>

        {message ? (
          <output
            className={`block rounded-lg p-3 text-center text-sm ${
              isSuccess
                ? "bg-brand-soft text-brand"
                : "border border-border bg-surface-elevated text-muted-foreground"
            }`}
          >
            {message}
          </output>
        ) : null}

        <Button type="submit" size="lg" className="w-full" disabled={isLoading || isSuccess}>
          {isLoading ? "Sending..." : "Join the Slack community"}
        </Button>

        <p className="text-center text-xs leading-relaxed text-muted">
          We use your address only to send the invite.
        </p>
      </form>
    </div>
  );
}
