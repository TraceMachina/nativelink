"use client";

import { Button } from "@nativelink/ui";
import { type FormEvent, useState } from "react";
import { workEmailError } from "../lib/book-download";
import { TRIAL_WEEKS, companyNameError } from "../lib/trial";

const inputClass =
  "w-full rounded-lg border border-border bg-background px-4 py-3 text-[15px] text-foreground placeholder:text-muted transition-colors focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/40 disabled:opacity-50";

/**
 * Work email and company name; emails the team, who issue the trial licence.
 * The visitor is told to expect it within a day, the site-wide promise.
 */
export function TrialRequestForm() {
  const [email, setEmail] = useState("");
  const [company, setCompany] = useState("");
  const [message, setMessage] = useState("");
  const [isSuccess, setIsSuccess] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setMessage("");
    setIsSuccess(false);

    const address = email.trim();
    const name = company.trim();
    const invalid = workEmailError(address) ?? companyNameError(name);
    if (invalid) {
      setMessage(invalid);
      return;
    }

    setIsLoading(true);
    try {
      const response = await fetch("/api/trial", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: address, company: name }),
      });
      const data = (await response.json().catch(() => ({}))) as { error?: string };
      if (!response.ok) {
        throw new Error(data.error || "Failed to send your request");
      }
      setIsSuccess(true);
      setMessage(
        `Thanks. We'll send ${name}'s ${TRIAL_WEEKS}-week trial licence to ${address}, usually within a day.`,
      );
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

  const locked = isLoading || isSuccess;

  return (
    <div className="rounded-2xl border border-border bg-surface p-8 shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.35)] md:p-10">
      <form onSubmit={handleSubmit} className="space-y-4">
        <div>
          <label
            className="mb-2 block font-mono text-[11px] uppercase tracking-[0.14em] text-muted"
            htmlFor="trial-email"
          >
            Work email
          </label>
          <input
            id="trial-email"
            name="email"
            type="email"
            placeholder="you@company.com"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            required
            autoComplete="email"
            disabled={locked}
            className={inputClass}
          />
        </div>
        <div>
          <label
            className="mb-2 block font-mono text-[11px] uppercase tracking-[0.14em] text-muted"
            htmlFor="trial-company"
          >
            Company
          </label>
          <input
            id="trial-company"
            name="company"
            type="text"
            placeholder="Acme Robotics"
            value={company}
            onChange={(event) => setCompany(event.target.value)}
            required
            maxLength={120}
            autoComplete="organization"
            disabled={locked}
            className={inputClass}
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

        <Button type="submit" size="lg" className="w-full" disabled={locked}>
          {isLoading ? "Sending..." : `Request a ${TRIAL_WEEKS}-week trial`}
        </Button>

        <p className="text-center text-xs leading-relaxed text-muted">
          We use these only to issue the trial and follow up on it.
        </p>
      </form>
    </div>
  );
}
