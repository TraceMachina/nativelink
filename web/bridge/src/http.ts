import { serve } from "bun";

export const startWebServer = () => {
    console.log('\nHTTP server is running on http://localhost:3001\n');
    serve({
      port: 3001,
      fetch(req) {
        const url = new URL(req.url);
        const handler = routes.get(url.pathname);
        if (handler){
            return handler();
        }
        return new Response("Not Found", { status: 404 });
      },
    });
};

const routes = new Map<string, () => Response>([
    ["/api", () => jsonResponse({ message: "Hello from API" })],
    ["/health", () => jsonResponse({ status: "ok" })],
    ["/readiness", () => jsonResponse({ status: "ready" })],
  ]);


function jsonResponse(data: object, status: number = 200): Response {
    const responseData = {
      ...data,
      timestamp: new Date().toISOString(),
    };
    return new Response(JSON.stringify(responseData), {
      headers: {
        "Content-Type": "application/json",
        "Access-Control-Allow-Origin": "*"
      },
      status,
    });
  }
