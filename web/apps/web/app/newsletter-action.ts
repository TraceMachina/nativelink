"use server";

import type { NewsletterState } from "@nativelink/ui";

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export async function subscribeNewsletter(
  _prev: NewsletterState | null,
  formData: FormData,
): Promise<NewsletterState> {
  const email = String(formData.get("email") ?? "").trim();

  if (!email) {
    return { ok: false, message: "Enter your email address.", error: true };
  }
  if (!EMAIL_RE.test(email)) {
    return { ok: false, message: "That doesn't look like a valid email.", error: true };
  }

  // In production this would forward to Resend Audiences / ConvertKit / etc.
  console.log("[newsletter] subscribe:", email);
  await new Promise((r) => setTimeout(r, 300));

  return {
    ok: true,
    message: "You're in. Check your inbox for confirmation.",
  };
}
