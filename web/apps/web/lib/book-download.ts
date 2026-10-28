export const BOOK_DOWNLOAD_URL = "https://endflakytests.com/OReilly_Extending_Bazel_Nativelink.pdf";

const MAX_EMAIL_LENGTH = 254;

// Markup and RFC 5322 specials are rejected outright. Domain labels exclude "." so the
// match stays linear; the length cap below bounds it regardless.
const EMAIL_REGEX = /^[^\s@<>()[\]\\,;:"]+@[^\s@.<>()[\]\\,;:"]+(?:\.[^\s@.<>()[\]\\,;:"]+)+$/;

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
  if (email.length > MAX_EMAIL_LENGTH || !EMAIL_REGEX.test(email)) {
    return "Please enter a valid email address";
  }
  const domain = email.slice(email.indexOf("@") + 1).toLowerCase();
  if (PERSONAL_EMAIL_DOMAINS.has(domain)) {
    return "Please use your work email address";
  }
  return null;
}
