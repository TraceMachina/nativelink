"use client";

import * as React from "react";
import { cn } from "../lib/cn";

export type NewsletterState = {
  ok: boolean;
  message: string;
  error?: boolean;
};

interface NewsletterFormProps {
  action: (
    prev: NewsletterState | null,
    formData: FormData,
  ) => Promise<NewsletterState> | NewsletterState;
  className?: string;
  inputClassName?: string;
  buttonClassName?: string;
  placeholder?: string;
  buttonLabel?: string;
}

export function NewsletterForm({
  action,
  className,
  inputClassName,
  buttonClassName,
  placeholder = "you@example.com",
  buttonLabel = "Subscribe",
}: NewsletterFormProps) {
  const [state, dispatch, pending] = React.useActionState<NewsletterState | null, FormData>(
    action,
    null,
  );

  return (
    <form action={dispatch} className={cn("flex flex-col gap-2", className)} noValidate>
      <fieldset disabled={pending} className="flex flex-col gap-2 disabled:opacity-70">
        <div className="flex gap-2">
          <label className="sr-only" htmlFor="newsletter-email">
            Email address
          </label>
          <input
            id="newsletter-email"
            name="email"
            type="email"
            required
            placeholder={placeholder}
            autoComplete="email"
            className={cn(
              "h-10 flex-1 rounded-lg border border-border bg-surface px-3 text-sm text-foreground transition-colors",
              "placeholder:text-muted focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/30",
              inputClassName,
            )}
          />
          <button
            type="submit"
            aria-busy={pending}
            className={cn(
              "inline-flex h-10 cursor-pointer items-center justify-center rounded-lg bg-brand px-4 text-sm font-medium text-brand-foreground transition-colors",
              "hover:bg-brand-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60",
              "disabled:cursor-wait",
              buttonClassName,
            )}
          >
            {pending ? (
              <span className="inline-flex items-center gap-2">
                <span className="h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Sending
              </span>
            ) : (
              buttonLabel
            )}
          </button>
        </div>
        {state ? (
          <p
            role={state.ok ? "status" : "alert"}
            className={cn(
              "font-mono text-xs",
              state.ok ? "text-success" : "text-amber-600 dark:text-amber-400",
            )}
          >
            {state.message}
          </p>
        ) : (
          <p className="font-mono text-[11px] text-muted">
            Build performance write-ups, occasionally. No spam.
          </p>
        )}
      </fieldset>
    </form>
  );
}
