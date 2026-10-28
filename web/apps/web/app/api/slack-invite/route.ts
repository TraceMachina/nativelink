import { type NextRequest, NextResponse } from "next/server";
import { emailFormatError } from "../../../lib/email";
import { sendSlackInviteRequest } from "../../../lib/mail";
import { createRateLimiter } from "../../../lib/rate-limit";

const allowRequest = createRateLimiter({ limit: 5, windowMs: 10 * 60 * 1000 });

// Vercel sets x-forwarded-for itself, so the first entry is the real client address.
function clientIp(request: NextRequest): string {
  return request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() || "unknown";
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

  const email =
    typeof body === "object" &&
    body !== null &&
    "email" in body &&
    typeof (body as { email: unknown }).email === "string"
      ? (body as { email: string }).email.trim()
      : "";

  const invalid = emailFormatError(email);
  if (invalid) {
    return NextResponse.json({ error: invalid }, { status: 400 });
  }

  try {
    await sendSlackInviteRequest({ email, time: new Date().toISOString() });
    return NextResponse.json({ message: "Invite requested" });
  } catch (error) {
    console.error("Slack invite request failed:", error);
    return NextResponse.json(
      { error: "We could not send your request. Please try again later." },
      { status: 500 },
    );
  }
}
