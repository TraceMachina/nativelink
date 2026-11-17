const MAX_COMPANY_LENGTH = 120;

/**
 * Returns the message to show, or null if the company name is usable. It
 * only has to be a short line of text: it goes into an email subject.
 */
export function companyNameError(company: string): string | null {
  const trimmed = company.trim();
  if (trimmed.length === 0) {
    return "Please enter your company name";
  }
  if (trimmed.length > MAX_COMPANY_LENGTH) {
    return `Company name must be ${MAX_COMPANY_LENGTH} characters or fewer`;
  }
  // Control characters (including newlines) have no place in a subject line.
  for (const ch of trimmed) {
    const code = ch.charCodeAt(0);
    if (code < 0x20 || code === 0x7f) {
      return "Please enter a valid company name";
    }
  }
  return null;
}

/** The Enterprise trial, as promised on the pricing page. */
export const TRIAL_WEEKS = 6;
