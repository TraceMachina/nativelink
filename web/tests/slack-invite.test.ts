import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import type { NextRequest } from "next/server";
import { POST } from "../apps/web/app/api/slack-invite/route";
import { emailFormatError } from "../apps/web/lib/email";
import { sendSlackInviteRequest } from "../apps/web/lib/mail";

const INVALID = "Please enter a valid email address";

describe("emailFormatError", () => {
  test("accepts any well-formed address, consumer domains included", () => {
    for (const email of [
      "a@b.com",
      "someone@gmail.com",
      "first.last@sub.corp.co.uk",
      "x+tag@a.io",
    ]) {
      expect(emailFormatError(email)).toBeNull();
    }
  });

  test("rejects malformed addresses and markup", () => {
    for (const email of ["", "a@b", "a@.com", "a b@c.com", "<svg/onload=alert(1)>@evil.com"]) {
      expect(emailFormatError(email)).toBe(INVALID);
    }
  });

  test("caps length before matching", () => {
    expect(emailFormatError(`${"a".repeat(250)}@b.com`)).toBe(INVALID);
    const attack = `!@!.${"!.".repeat(50_000)}\n`;
    const start = performance.now();
    expect(emailFormatError(attack)).toBe(INVALID);
    expect(performance.now() - start).toBeLessThan(50);
  });
});

type FetchCall = { url: string; body: Record<string, unknown> };

function mockFetch(responses: Record<string, () => Response>) {
  const calls: FetchCall[] = [];
  globalThis.fetch = mock(async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, body: JSON.parse(String(init?.body)) });
    const path = new URL(url).pathname;
    return responses[path]?.() ?? new Response(null, { status: 500 });
  }) as unknown as typeof fetch;
  return calls;
}

const ok = () => Response.json({ id: "1" });
const realFetch = globalThis.fetch;
const savedEnv = { ...process.env };

beforeEach(() => {
  process.env.RESEND_API_KEY = "re_test";
});

afterEach(() => {
  globalThis.fetch = realFetch;
  process.env = { ...savedEnv };
});

describe("sendSlackInviteRequest", () => {
  const request = { email: "<b>x</b>@example.com", time: "2026-10-28T00:00:00.000Z" };

  test("emails contact@tracemachina.com as plain text, with the address in the subject", async () => {
    const calls = mockFetch({ "/emails": ok });
    await sendSlackInviteRequest(request);
    expect(calls.map((c) => new URL(c.url).pathname)).toEqual(["/emails"]);
    expect(calls[0].body.to).toEqual(["contact@tracemachina.com"]);
    expect(calls[0].body.subject).toBe(`Slack invite request: ${request.email}`);
    expect(calls[0].body.html).toBeUndefined();
    expect(calls[0].body.text).toContain(request.email);
    expect(calls[0].body.text).toContain(request.time);
  });

  test("fails when the email cannot be sent, because the email is the whole action", async () => {
    mockFetch({ "/emails": () => new Response(null, { status: 500 }) });
    await expect(sendSlackInviteRequest(request)).rejects.toThrow("Failed to send Slack invite");
  });

  test("requires RESEND_API_KEY", async () => {
    Reflect.deleteProperty(process.env, "RESEND_API_KEY");
    await expect(sendSlackInviteRequest(request)).rejects.toThrow("RESEND_API_KEY");
  });
});

describe("POST /api/slack-invite", () => {
  let ip = 0;
  function post(body: string, clientIp = `10.1.0.${++ip}`) {
    return POST(
      new Request("http://localhost/api/slack-invite", {
        method: "POST",
        headers: { "Content-Type": "application/json", "x-forwarded-for": clientIp },
        body,
      }) as unknown as NextRequest,
    );
  }

  test("rejects invalid JSON and malformed addresses with 400", async () => {
    expect((await post("{")).status).toBe(400);
    const markup = await post(JSON.stringify({ email: "<svg/onload=alert(1)>@evil.com" }));
    expect(markup.status).toBe(400);
    expect((await markup.json()).error).toBe(INVALID);
  });

  test("accepts a consumer address and returns 200 once the team has been emailed", async () => {
    const calls = mockFetch({ "/emails": ok });
    const res = await post(JSON.stringify({ email: " someone@gmail.com " }));
    expect(res.status).toBe(200);
    expect(calls[0].body.subject).toBe("Slack invite request: someone@gmail.com");
  });

  test("returns 500 when Resend fails or is not configured", async () => {
    mockFetch({ "/emails": () => new Response(null, { status: 500 }) });
    expect((await post(JSON.stringify({ email: "someone@example.com" }))).status).toBe(500);
    Reflect.deleteProperty(process.env, "RESEND_API_KEY");
    expect((await post(JSON.stringify({ email: "someone@example.com" }))).status).toBe(500);
  });

  test("returns 429 after five requests from one address", async () => {
    const body = JSON.stringify({ email: "not-an-email" });
    for (let i = 0; i < 5; i++) {
      expect((await post(body, "192.0.2.10")).status).toBe(400);
    }
    expect((await post(body, "192.0.2.10")).status).toBe(429);
    expect((await post(body, "192.0.2.11")).status).toBe(400);
  });
});
