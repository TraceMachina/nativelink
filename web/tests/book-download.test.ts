import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import type { NextRequest } from "next/server";
import { POST } from "../apps/web/app/api/mail/route";
import { workEmailError } from "../apps/web/lib/book-download";
import { recordBookDownload } from "../apps/web/lib/mail";
import { createRateLimiter } from "../apps/web/lib/rate-limit";

const INVALID = "Please enter a valid email address";
const PERSONAL = "Please use your work email address";

describe("workEmailError", () => {
  test("accepts work addresses", () => {
    for (const email of [
      "a@b.com",
      "first.last@sub.corp.co.uk",
      "x+tag@a.io",
      "o'brien@corp.com",
    ]) {
      expect(workEmailError(email)).toBeNull();
    }
  });

  test("rejects consumer domains case-insensitively", () => {
    expect(workEmailError("someone@gmail.com")).toBe(PERSONAL);
    expect(workEmailError("someone@GMAIL.COM")).toBe(PERSONAL);
  });

  test("rejects malformed addresses", () => {
    for (const email of ["", "a@b", "a@.com", "a@b.", "a@b..com", "a b@c.com", "a@@b.com"]) {
      expect(workEmailError(email)).toBe(INVALID);
    }
  });

  test("rejects markup in the address", () => {
    expect(workEmailError("<svg/onload=alert(1)>@evil.com")).toBe(INVALID);
    expect(workEmailError('"><img/src=x>@evil.com')).toBe(INVALID);
  });

  test("caps length before matching, so the CodeQL input shape stays cheap", () => {
    expect(workEmailError(`${"a".repeat(250)}@b.com`)).toBe(INVALID);
    const attack = `!@!.${"!.".repeat(50_000)}\n`;
    const start = performance.now();
    expect(workEmailError(attack)).toBe(INVALID);
    expect(performance.now() - start).toBeLessThan(50);
  });
});

describe("createRateLimiter", () => {
  test("allows up to the limit per key within a window, then resets", () => {
    const allow = createRateLimiter({ limit: 2, windowMs: 1000 });
    expect(allow("ip", 0)).toBe(true);
    expect(allow("ip", 10)).toBe(true);
    expect(allow("ip", 20)).toBe(false);
    expect(allow("other", 20)).toBe(true);
    expect(allow("ip", 1000)).toBe(true);
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
  Reflect.deleteProperty(process.env, "RESEND_SEGMENT_ID");
});

afterEach(() => {
  globalThis.fetch = realFetch;
  process.env = { ...savedEnv };
});

describe("recordBookDownload", () => {
  const download = { email: "<b>x</b>@corp.com", time: "2026-10-26T00:00:00.000Z" };

  test("stores the contact and sends only a plain-text internal notification", async () => {
    process.env.RESEND_SEGMENT_ID = "seg_1";
    const calls = mockFetch({ "/contacts": ok, "/emails": ok });
    await recordBookDownload(download);

    expect(calls.map((c) => new URL(c.url).pathname)).toEqual(["/contacts", "/emails"]);
    expect(calls[0].body).toEqual({
      email: download.email,
      unsubscribed: false,
      segments: [{ id: "seg_1" }],
    });
    expect(calls[1].body.to).toEqual(["hello@nativelink.com"]);
    expect(calls[1].body.html).toBeUndefined();
    expect(calls[1].body.text).toContain(download.email);
  });

  test("treats an existing contact as stored", async () => {
    mockFetch({ "/contacts": () => new Response(null, { status: 409 }), "/emails": ok });
    await expect(recordBookDownload(download)).resolves.toBeUndefined();
  });

  test("fails when the contact cannot be stored", async () => {
    const calls = mockFetch({ "/contacts": () => new Response(null, { status: 500 }) });
    await expect(recordBookDownload(download)).rejects.toThrow("Failed to store contact");
    expect(calls).toHaveLength(1);
  });

  test("a failed notification does not fail the download", async () => {
    mockFetch({ "/contacts": ok, "/emails": () => new Response(null, { status: 500 }) });
    await expect(recordBookDownload(download)).resolves.toBeUndefined();
  });

  test("requires RESEND_API_KEY", async () => {
    Reflect.deleteProperty(process.env, "RESEND_API_KEY");
    await expect(recordBookDownload(download)).rejects.toThrow("RESEND_API_KEY");
  });
});

describe("POST /api/mail", () => {
  let ip = 0;
  function post(body: string, clientIp = `10.0.0.${++ip}`) {
    return POST(
      new Request("http://localhost/api/mail", {
        method: "POST",
        headers: { "Content-Type": "application/json", "x-forwarded-for": clientIp },
        body,
      }) as unknown as NextRequest,
    );
  }

  test("rejects invalid JSON, markup and consumer domains with 400", async () => {
    expect((await post("{")).status).toBe(400);
    const markup = await post(JSON.stringify({ email: "<svg/onload=alert(1)>@evil.com" }));
    expect(markup.status).toBe(400);
    expect((await markup.json()).error).toBe(INVALID);
    const personal = await post(JSON.stringify({ email: "me@gmail.com" }));
    expect((await personal.json()).error).toBe(PERSONAL);
  });

  test("returns 200 once the contact is stored", async () => {
    mockFetch({ "/contacts": ok, "/emails": ok });
    expect((await post(JSON.stringify({ email: " reader@corp.com " }))).status).toBe(200);
  });

  test("returns 500 when Resend is not configured", async () => {
    Reflect.deleteProperty(process.env, "RESEND_API_KEY");
    expect((await post(JSON.stringify({ email: "reader@corp.com" }))).status).toBe(500);
  });

  test("returns 429 after five requests from one address", async () => {
    const body = JSON.stringify({ email: "me@gmail.com" });
    for (let i = 0; i < 5; i++) {
      expect((await post(body, "192.0.2.1")).status).toBe(400);
    }
    expect((await post(body, "192.0.2.1")).status).toBe(429);
    expect((await post(body, "192.0.2.2")).status).toBe(400);
  });
});
