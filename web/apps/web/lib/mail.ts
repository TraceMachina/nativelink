export interface EmailData {
  email: string;
  time: string;
}

const subscriptionTemplate = (data: EmailData): string => `
  <p>New book download from <strong>${data.email}</strong></p>
  <p>Time: ${data.time}</p>
`;

const thanksTemplate = (): string => `
  <p>Hello,</p>
  <p>Thank you for downloading <em>Extending Bazel to Its Full Potential</em>.</p>
  <p>We will get back to you as soon as possible if you have questions.</p>
  <p>Best regards,<br>The NativeLink Team</p>
`;

/**
 * Notify the team and thank the reader via Resend.
 * Requires `RESEND_API_KEY` in the marketing app environment.
 */
export async function sendBookDownloadMails(data: EmailData): Promise<void> {
  const apiKey = process.env.RESEND_API_KEY;
  if (!apiKey) {
    throw new Error("RESEND_API_KEY is not configured");
  }

  const recipients = [
    {
      email: data.email,
      subject: "Thank you for your download",
      html: thanksTemplate(),
    },
    {
      email: "hello@nativelink.com",
      subject: "New O'Reilly book download",
      html: subscriptionTemplate(data),
    },
  ];

  for (const recipient of recipients) {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        from: "Nativelink System <system@nativelink.com>",
        to: [recipient.email],
        subject: recipient.subject,
        html: recipient.html,
      }),
    });

    if (!res.ok) {
      const errorData = await res.json().catch(() => null);
      console.error("Failed to send email:", errorData);
      throw new Error(`Failed to send email: ${res.statusText}`);
    }
  }
}
