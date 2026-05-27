"use client";

import { Button, cn } from "@nativelink/ui";
import { useActionState } from "react";
import { type ContactState, submitContact } from "./actions";

const initial: ContactState | null = null;

const topics = [
  { value: "sales", label: "Sales — talk to a solutions engineer" },
  { value: "support", label: "Support — I'm an existing customer" },
  { value: "partnership", label: "Partnership / integration" },
  { value: "press", label: "Press / media" },
  { value: "general", label: "General inquiry" },
];

export function ContactForm() {
  const [state, action, pending] = useActionState(submitContact, initial);

  if (state?.ok) {
    return (
      <div className="rounded-2xl border border-success/40 bg-success-soft/60 p-8 text-center">
        <div className="mx-auto mb-4 inline-flex h-12 w-12 items-center justify-center rounded-full bg-success text-background">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M5 12 L10 17 L20 7" />
          </svg>
        </div>
        <p className="text-lg font-semibold text-foreground">Message sent</p>
        <p className="mt-2 text-[15px] text-muted-foreground">{state.message}</p>
      </div>
    );
  }

  const err = state?.errors;

  return (
    <form action={action} className="space-y-5" noValidate>
      {/* fieldset disabled={pending} locks every nested input + select +
       *  textarea + button in one go while the server action runs. */}
      <fieldset disabled={pending} className="space-y-5 disabled:opacity-60">
        <div className="grid gap-5 sm:grid-cols-2">
          <Field
            label="Your name"
            name="name"
            autoComplete="name"
            required
            error={err?.name}
          />
          <Field
            label="Email"
            name="email"
            type="email"
            autoComplete="email"
            required
            error={err?.email}
          />
        </div>

        <div className="flex flex-col gap-2">
          <label htmlFor="topic" className="font-mono text-xs uppercase tracking-[0.14em] text-muted-foreground">
            What's this about?
          </label>
          <select
            id="topic"
            name="topic"
            defaultValue=""
            className={cn(
              "h-11 cursor-pointer rounded-lg border bg-surface px-3 text-[15px] text-foreground transition-colors",
              "focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/30",
              "disabled:cursor-wait",
              err?.topic ? "border-amber-500/60" : "border-border",
            )}
          >
            <option value="" disabled>
              Pick a topic
            </option>
            {topics.map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </select>
          {err?.topic ? (
            <p className="font-mono text-xs text-amber-600 dark:text-amber-400">{err.topic}</p>
          ) : null}
        </div>

        <div className="flex flex-col gap-2">
          <label htmlFor="message" className="font-mono text-xs uppercase tracking-[0.14em] text-muted-foreground">
            Tell us more
          </label>
          <textarea
            id="message"
            name="message"
            rows={6}
            required
            minLength={10}
            placeholder="What are you building? Where does NativeLink fit?"
            className={cn(
              "rounded-lg border bg-surface px-3 py-3 text-[15px] leading-relaxed text-foreground transition-colors",
              "placeholder:text-muted/70 focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/30",
              "disabled:cursor-wait",
              err?.message ? "border-amber-500/60" : "border-border",
            )}
          />
          {err?.message ? (
            <p className="font-mono text-xs text-amber-600 dark:text-amber-400">{err.message}</p>
          ) : null}
        </div>

        {state && !state.ok && !err ? (
          <p className="rounded-lg border border-amber-500/40 bg-amber-50/40 px-4 py-3 text-sm text-amber-700 dark:bg-amber-500/10 dark:text-amber-300">
            {state.message}
          </p>
        ) : null}

        <div className="flex flex-wrap items-center justify-between gap-4">
          <p className="font-mono text-xs text-muted">
            We respond within 1 business day. Usually faster.
          </p>
          <Button type="submit" size="lg" aria-busy={pending}>
            {pending ? (
              <span className="inline-flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Sending…
              </span>
            ) : (
              "Send message"
            )}
          </Button>
        </div>
      </fieldset>
    </form>
  );
}

interface FieldProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label: string;
  error?: string;
}

function Field({ label, name, error, ...props }: FieldProps) {
  return (
    <div className="flex flex-col gap-2">
      <label
        htmlFor={name}
        className="font-mono text-xs uppercase tracking-[0.14em] text-muted-foreground"
      >
        {label}
      </label>
      <input
        id={name}
        name={name}
        className={cn(
          "h-11 rounded-lg border bg-surface px-3 text-[15px] text-foreground transition-colors",
          "placeholder:text-muted/70 focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/30",
          error ? "border-amber-500/60" : "border-border",
        )}
        {...props}
      />
      {error ? (
        <p className="font-mono text-xs text-amber-600 dark:text-amber-400">{error}</p>
      ) : null}
    </div>
  );
}
