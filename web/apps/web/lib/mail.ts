const RESEND_API = "https://api.resend.com";
const NOTIFY_ADDRESS = "hello@nativelink.com";
const COMMUNITY_ADDRESS = "contact@tracemachina.com";

export interface BookDownload {
  email: string;
  time: string;
}

function resend(path: string, apiKey: string, body: unknown): Promise<Response> {
  return fetch(`${RESEND_API}${path}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify(body),
  });
}

async function isExistingContact(res: Response): Promise<boolean> {
  if (res.status === 409) {
    return true;
  }
  const error = (await res.json().catch(() => null)) as { message?: unknown } | null;
  return typeof error?.message === "string" && /already exists/i.test(error.message);
}

/**
 * Store the reader as a Resend contact, then notify the team. Storing must succeed;
 * the notification is best-effort so a dropped internal email never costs a download.
 * Requires `RESEND_API_KEY`; `RESEND_SEGMENT_ID` optionally files the contact under a segment.
 */
export async function recordBookDownload({ email, time }: BookDownload): Promise<void> {
  const apiKey = process.env.RESEND_API_KEY;
  if (!apiKey) {
    throw new Error("RESEND_API_KEY is not configured");
  }

  const segmentId = process.env.RESEND_SEGMENT_ID;
  const contact = await resend("/contacts", apiKey, {
    email,
    unsubscribed: false,
    ...(segmentId ? { segments: [{ id: segmentId }] } : {}),
  });
  if (!contact.ok && !(await isExistingContact(contact))) {
    throw new Error(`Failed to store contact: ${contact.status} ${contact.statusText}`);
  }

  try {
    // Plain text: the address is caller-controlled and must never be rendered as markup.
    const notification = await resend("/emails", apiKey, {
      from: "NativeLink System <system@nativelink.io>",
      to: [NOTIFY_ADDRESS],
      subject: "New O'Reilly book download",
      text: `New book download from ${email}\nTime: ${time}\n`,
    });
    if (!notification.ok) {
      console.error("Failed to send download notification:", notification.status);
    }
  } catch (error) {
    console.error("Failed to send download notification:", error);
  }
}

export interface SlackInviteRequest {
  email: string;
  time: string;
}

/**
 * Ask the team to invite someone to the community Slack. Unlike the book
 * download there is nothing to hand the visitor afterwards, so the email to
 * the team is the whole action and a failure to send is a failure.
 * Requires `RESEND_API_KEY`.
 */
export async function sendSlackInviteRequest({ email, time }: SlackInviteRequest): Promise<void> {
  const apiKey = process.env.RESEND_API_KEY;
  if (!apiKey) {
    throw new Error("RESEND_API_KEY is not configured");
  }
  // Plain text: the address is caller-controlled and must never be rendered as markup.
  const res = await resend("/emails", apiKey, {
    from: "NativeLink System <system@nativelink.io>",
    to: [COMMUNITY_ADDRESS],
    subject: `Slack invite request: ${email}`,
    text: `${email} asked to join the NativeLink Slack community.\nTime: ${time}\n\nSend them an invite from the Slack admin console.\n`,
  });
  if (!res.ok) {
    throw new Error(`Failed to send Slack invite request: ${res.status} ${res.statusText}`);
  }
}
