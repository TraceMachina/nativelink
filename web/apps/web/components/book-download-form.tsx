"use client";

import { Button } from "@nativelink/ui";
import { type FormEvent, useState } from "react";
import { BOOK_DOWNLOAD_URL, workEmailError } from "../lib/book-download";

export function BookDownloadForm() {
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [isSuccess, setIsSuccess] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setMessage("");
    setIsSuccess(false);

    const invalid = workEmailError(email.trim());
    if (invalid) {
      setMessage(invalid);
      return;
    }

    setIsLoading(true);
    try {
      const response = await fetch("/api/mail", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email }),
      });
      const data = (await response.json().catch(() => ({}))) as { error?: string };

      if (!response.ok) {
        throw new Error(data.error || "Failed to process request");
      }

      setIsSuccess(true);
      setMessage("Success! Redirecting to your download...");
      setEmail("");
      window.setTimeout(() => {
        window.location.href = BOOK_DOWNLOAD_URL;
      }, 1000);
    } catch (error) {
      setIsSuccess(false);
      setMessage(
        error instanceof Error && error.message
          ? error.message
          : "An unexpected error occurred. Please try again later.",
      );
      console.error("Error processing download request:", error);
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <div className="mx-auto max-w-md rounded-2xl border border-border bg-surface p-8 shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.35)] md:p-10">
      <h3 className="text-center text-2xl font-semibold tracking-tight text-foreground md:text-3xl">
        Get your free copy
      </h3>
      <p className="mt-3 text-center text-[15px] leading-relaxed text-muted-foreground">
        Enter your work email to download the book
      </p>

      <form onSubmit={handleSubmit} className="mt-8 space-y-4">
        <div>
          <label
            className="mb-2 block font-mono text-[11px] uppercase tracking-[0.14em] text-muted"
            htmlFor="book-email"
          >
            Work email address *
          </label>
          <input
            id="book-email"
            name="email"
            type="email"
            placeholder="you@company.com"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            required
            autoComplete="email"
            disabled={isLoading}
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

        <Button type="submit" size="lg" className="w-full" disabled={isLoading}>
          {isLoading ? "Processing..." : "Download book"}
        </Button>

        <p className="text-center text-xs leading-relaxed text-muted">
          By downloading, you agree to receive occasional updates from NativeLink. You can
          unsubscribe at any time.
        </p>
      </form>
    </div>
  );
}
