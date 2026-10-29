import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import type { NextRequest } from "next/server";
import { POST } from "../apps/web/app/api/trial/route";
import { sendTrialRequest } from "../apps/web/lib/mail";
import { companyNameError } from "../apps/web/lib/trial";

describe("companyNameError", () => {
  test("accepts ordinary company names", () => {
    for (const name of ["Acme", "Trace Machina, Inc.", "O'Reilly & Sons", "株式会社"]) {
      expect(companyNameError(name)).toBeNull();
    }
  });

  test("rejects empty, over-long and control-character names", () => {
    expect(companyNameError("")).toBe("Please enter your company name");
    expect(companyNameError("   ")).toBe("Please enter your company name");
    expect(companyNameError("a".repeat(121))).toContain("120 characters");
    expect(companyNameError("Acme\nBcc: evil@example.com")).toBe(
      "Please enter a valid company name",
    );
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

describe("sendTrialRequest", () => {
  const request = {
    email: "cto@acme.example",
    company: "<b>Acme</b>",
    time: "2026-10-29T00:00:00.000Z",
  };

  test("emails contact@tracemachina.com with the company in the subject, as plain text", async () => {
    const calls = mockFetch({ "/emails": ok });
    await sendTrialRequest(request);
    expect(calls[0].body.to).toEqual(["contact@tracemachina.com"]);
    expect(calls[0].body.subject).toBe("<b>Acme</b> requesting NativeLink Trial");
    expect(calls[0].body.html).toBeUndefined();
    expect(calls[0].body.text).toContain(request.email);
  });

  test("fails when the email cannot be sent", async () => {
    mockFetch({ "/emails": () => new Response(null, { status: 500 }) });
    await expect(sendTrialRequest(request)).rejects.toThrow("Failed to send trial request");
  });
});

describe("POST /api/trial", () => {
  let ip = 0;
  function post(body: string, clientIp = `10.2.0.${++ip}`) {
    return POST(
      new Request("http://localhost/api/trial", {
        method: "POST",
        headers: { "Content-Type": "application/json", "x-forwarded-for": clientIp },
        body,
      }) as unknown as NextRequest,
    );
  }

  test("rejects bad JSON, consumer addresses and missing companies with 400", async () => {
    expect((await post("{")).status).toBe(400);
    const personal = await post(JSON.stringify({ email: "me@gmail.com", company: "Acme" }));
    expect(personal.status).toBe(400);
    expect((await personal.json()).error).toBe("Please use your work email address");
    const noCompany = await post(JSON.stringify({ email: "cto@acme.example" }));
    expect(noCompany.status).toBe(400);
    expect((await noCompany.json()).error).toBe("Please enter your company name");
  });

  test("returns 200 once the team has been emailed", async () => {
    const calls = mockFetch({ "/emails": ok });
    const res = await post(JSON.stringify({ email: " cto@acme.example ", company: " Acme " }));
    expect(res.status).toBe(200);
    expect(calls[0].body.subject).toBe("Acme requesting NativeLink Trial");
  });

  test("returns 500 when Resend fails", async () => {
    mockFetch({ "/emails": () => new Response(null, { status: 500 }) });
    expect(
      (await post(JSON.stringify({ email: "cto@acme.example", company: "Acme" }))).status,
    ).toBe(500);
  });

  test("returns 429 after five requests from one address", async () => {
    const body = JSON.stringify({ email: "nope", company: "Acme" });
    for (let i = 0; i < 5; i++) {
      expect((await post(body, "192.0.2.20")).status).toBe(400);
    }
    expect((await post(body, "192.0.2.20")).status).toBe(429);
  });
});
