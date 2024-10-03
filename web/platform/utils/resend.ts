/// <reference path="./deno.d.ts" />
import {
  type EmailData,
  subscription,
  thanks,
} from "../src/content/mails/template";

const isDeno = typeof Deno !== "undefined";

let env: Record<string, string | undefined> = {};

if (isDeno) {
  // In Deno, only set API key from Deno.env.get
  env.RESEND_API_KEY = Deno.env.get("RESEND_API_KEY");
  console.info("Running in Deno.");
} else {
  // In local development (non-Deno), use dotenv to load all variables
  const dotenv = await import("dotenv");
  const dotenvResult = dotenv.config();

  if (dotenvResult.error) {
    console.error("Error loading .env file:", dotenvResult.error);
  } else if (dotenvResult.parsed) {
    env = dotenvResult.parsed;
    console.info("Loaded .env file.");
  }
}

// Function to send emails using Resend API directly with fetch
export const getRe = () => {
  return {
    send: async (data: EmailData): Promise<void> => {
      const recipients = [
        {
          email: data.email,
          template: thanks(),
          subject: "Thank you for your subscription",
        },
        {
          email: "hello@nativelink.com",
          template: subscription(data),
          subject: "New subscription",
        },
      ];

      try {
        for (const recipient of recipients) {
          const res = await fetch("https://api.resend.com/emails", {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              Authorization: `Bearer ${env.RESEND_API_KEY}`,
            },
            body: JSON.stringify({
              from: "Nativelink System <system@nativelink.com>",
              to: [recipient.email],
              subject: recipient.subject,
              html: recipient.template,
            }),
          });

          if (!res.ok) {
            const errorData = await res.json();
            console.error("Failed to send email:", errorData);
            throw new Error(`Failed to send email: ${res.statusText}`);
          }
        }
      } catch (error) {
        console.error("Error sending emails:", error);
        throw error;
      }
    },
  };
};
