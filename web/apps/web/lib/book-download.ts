export const BOOK_DOWNLOAD_URL = "https://endflakytests.com/OReilly_Extending_Bazel_Nativelink.pdf";

// Domain labels exclude "." so the match is linear; the old form backtracked on "a@b.b.b...".
export const EMAIL_REGEX = /^[^\s@]+@[^\s@.]+(?:\.[^\s@.]+)+$/;

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

export function isWorkEmail(email: string): boolean {
  if (!EMAIL_REGEX.test(email)) {
    return false;
  }
  const domain = email.split("@")[1]?.toLowerCase();
  return Boolean(domain && !PERSONAL_EMAIL_DOMAINS.has(domain));
}
