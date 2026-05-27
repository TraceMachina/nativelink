"use server";

export type ContactState = {
  ok: boolean;
  message: string;
  errors?: Partial<Record<"name" | "email" | "topic" | "message", string>>;
};

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export async function submitContact(
  _prev: ContactState | null,
  formData: FormData,
): Promise<ContactState> {
  const name = String(formData.get("name") ?? "").trim();
  const email = String(formData.get("email") ?? "").trim();
  const topic = String(formData.get("topic") ?? "").trim();
  const message = String(formData.get("message") ?? "").trim();

  const errors: ContactState["errors"] = {};
  if (!name) errors.name = "Your name is required.";
  if (!email) errors.email = "Email is required.";
  else if (!EMAIL_RE.test(email)) errors.email = "Enter a valid email address.";
  if (!topic) errors.topic = "Pick a topic so we can route this correctly.";
  if (!message || message.length < 10)
    errors.message = "Tell us a bit more — at least a sentence or two.";

  if (Object.keys(errors).length > 0) {
    return { ok: false, message: "Please fix the highlighted fields.", errors };
  }

  // In production this would forward to a webhook / email service / CRM.
  // For now we log on the server and return success.
  console.log("[contact-form]", { name, email, topic, message });

  // Simulate small latency for UX feedback.
  await new Promise((r) => setTimeout(r, 400));

  return {
    ok: true,
    message: `Thanks ${name.split(" ")[0]} — we'll be in touch within one business day.`,
  };
}
