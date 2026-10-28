export const BOOK_DOWNLOAD_URL = "https://endflakytests.com/OReilly_Extending_Bazel_Nativelink.pdf";

import { emailFormatError } from "./email";

/** Consumer domains rejected for the gated book download. */
export const PERSONAL_EMAIL_DOMAINS = new Set([
  "gmail.com",
  "yahoo.com",
  "hotmail.com",
  "outlook.com",
  "aol.com",
  "icloud.com",
  "mail.com",
  "protonmail.com",
  "yandex.com",
  "zoho.com",
]);

/** Returns the message to show the reader, or null if the address is an acceptable work email. */
export function workEmailError(email: string): string | null {
  const malformed = emailFormatError(email);
  if (malformed) {
    return malformed;
  }
  const domain = email.slice(email.indexOf("@") + 1).toLowerCase();
  if (PERSONAL_EMAIL_DOMAINS.has(domain)) {
    return "Please use your work email address";
  }
  return null;
}
