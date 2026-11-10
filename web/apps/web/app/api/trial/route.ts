import { type NextRequest, NextResponse } from "next/server";
import { workEmailError } from "../../../lib/book-download";
import { sendTrialRequest } from "../../../lib/mail";
import { createRateLimiter } from "../../../lib/rate-limit";
import { companyNameError } from "../../../lib/trial";

const allowRequest = createRateLimiter({ limit: 5, windowMs: 10 * 60 * 1000 });

// Vercel sets x-forwarded-for itself, so the first entry is the real client address.
function clientIp(request: NextRequest): string {
  return request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() || "unknown";
}

function field(body: unknown, name: string): string {
  if (typeof body !== "object" || body === null || !(name in body)) {
    return "";
  }
  const value = (body as Record<string, unknown>)[name];
  return typeof value === "string" ? value.trim() : "";
}

export async function POST(request: NextRequest) {
  if (!allowRequest(clientIp(request))) {
    return NextResponse.json(
      { error: "Too many requests. Please try again later." },
      { status: 429 },
    );
  }

  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return NextResponse.json({ error: "Invalid JSON body" }, { status: 400 });
  }

  const email = field(body, "email");
  const company = field(body, "company");

  // A trial is for a company, so the work-email rule from the book download applies here too.
  const invalid = workEmailError(email) ?? companyNameError(company);
  if (invalid) {
    return NextResponse.json({ error: invalid }, { status: 400 });
  }

  try {
    await sendTrialRequest({ email, company, time: new Date().toISOString() });
    return NextResponse.json({ message: "Trial requested" });
  } catch (error) {
    console.error("Trial request failed:", error);
    return NextResponse.json(
      { error: "We could not send your request. Please try again later." },
      { status: 500 },
    );
  }
}
