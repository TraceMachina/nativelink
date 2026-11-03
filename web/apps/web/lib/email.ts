const MAX_EMAIL_LENGTH = 254;

// Markup and RFC 5322 specials are rejected outright. Domain labels exclude "." so the
// match stays linear; the length cap below bounds it regardless.
const EMAIL_REGEX = /^[^\s@<>()[\]\\,;:"]+@[^\s@.<>()[\]\\,;:"]+(?:\.[^\s@.<>()[\]\\,;:"]+)+$/;

/** Returns the message to show, or null if the address is well formed. Any domain is fine. */
export function emailFormatError(email: string): string | null {
  if (email.length > MAX_EMAIL_LENGTH || !EMAIL_REGEX.test(email)) {
    return "Please enter a valid email address";
  }
  return null;
}
