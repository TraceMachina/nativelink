import type { APIRoute } from "astro";
import { getRe } from "../../../utils/resend.ts";

export const POST: APIRoute = async ({ request }) => {
  if (request.headers.get("Content-Type") === "application/json") {
    const data = await request.json();
    const re = await getRe();

    data.time = new Date().toISOString();

    try {
      await re.send(data);

      return new Response(
        JSON.stringify({
          message: "Successfully subscribed!",
        }),
        {
          status: 200,
        },
      );
    } catch (error) {
      console.error("Error sending mails:", error);
      return new Response(
        JSON.stringify({
          error: "Failed to subscribe. Please try again later.",
        }),
        {
          status: 500,
        },
      );
    }
  }

  return new Response(null, { status: 400 });
};
