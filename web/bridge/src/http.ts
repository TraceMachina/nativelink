import { serve } from "bun";
import { build_data } from "./db"

const httpPort = Number(process.env.HTTP_PORT) || 3001;

export const startWebServer = () => {
    console.log(`\nHTTP server is running on http://localhost:${httpPort}\n`);
    serve({
      port: httpPort,
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

const routes = new Map<string, () => Promise<Response>>([
    ["/api", () => jsonResponse({ message: "Hello from API" })],
    ["/builds", () => getBuilds()],
    ["/health", () => jsonResponse({ status: "ok" })],
    ["/readiness", () => jsonResponse({ status: "ready" })],
  ]);

async function getBuilds(): Promise<Response> {
  const dbResult = await build_data;
  return jsonResponse({ build_data: dbResult})
}

async function jsonResponse(data: object, status: number = 200): Promise<Response> {
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
