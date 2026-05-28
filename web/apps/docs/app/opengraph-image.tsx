/* eslint-disable @next/next/no-img-element */
import { ImageResponse } from "next/og";

export const runtime = "edge";
export const alt = "NativeLink Documentation";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

export default async function OpenGraphImage() {
  return new ImageResponse(
    <div
      style={{
        height: "100%",
        width: "100%",
        display: "flex",
        flexDirection: "column",
        padding: "80px",
        position: "relative",
        background: "#0A090E",
        color: "#F5F4F8",
        fontFamily: "system-ui, -apple-system, sans-serif",
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "radial-gradient(ellipse 900px 600px at 15% -10%, rgba(156,124,255,0.32), transparent 60%), radial-gradient(ellipse 600px 400px at 100% 100%, rgba(156,124,255,0.18), transparent 60%)",
        }}
      />
      <div
        style={{
          position: "absolute",
          inset: 0,
          opacity: 0.15,
          backgroundImage: "radial-gradient(#9C7CFF 1px, transparent 1px)",
          backgroundSize: "32px 32px",
          maskImage: "radial-gradient(ellipse at top, black 30%, transparent 70%)",
        }}
      />

      <div
        style={{ display: "flex", alignItems: "center", gap: 16, position: "relative", zIndex: 1 }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            width: 64,
            height: 64,
            borderRadius: 16,
            background: "linear-gradient(135deg, #9C7CFF 0%, #BCA2FF 100%)",
            boxShadow: "0 12px 32px -8px rgba(156,124,255,0.55)",
          }}
        >
          {/* biome-ignore lint/a11y/noSvgWithoutTitle: rendered to a PNG by satori; there is no accessibility tree in the output image */}
          <svg
            width="40"
            height="40"
            viewBox="0 0 24 24"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
          >
            <path
              d="M5.6 6.5 H8.3 V13.8 L15.2 6.5 H18.4 V17.5 H15.7 V10.2 L8.8 17.5 H5.6 Z"
              fill="#0A090E"
            />
          </svg>
        </div>
        <div style={{ fontSize: 32, fontWeight: 600, letterSpacing: "-0.02em", color: "#F5F4F8" }}>
          nativelink
        </div>
        <div
          style={{
            marginLeft: 12,
            paddingLeft: 16,
            borderLeft: "1px solid #3C3A4E",
            fontSize: 24,
            fontWeight: 500,
            color: "#9496A2",
          }}
        >
          docs
        </div>
      </div>

      <div
        style={{
          marginTop: "auto",
          display: "flex",
          flexDirection: "column",
          gap: 24,
          position: "relative",
          zIndex: 1,
        }}
      >
        <div
          style={{
            fontSize: 78,
            fontWeight: 600,
            letterSpacing: "-0.04em",
            lineHeight: 1.05,
            maxWidth: 1000,
          }}
        >
          Documentation for{" "}
          <span
            style={{
              background: "linear-gradient(90deg, #9C7CFF 0%, #BCA2FF 100%)",
              backgroundClip: "text",
              WebkitBackgroundClip: "text",
              color: "transparent",
            }}
          >
            engineers who build the builds.
          </span>
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 24,
            fontSize: 22,
            color: "#9496A2",
            letterSpacing: "0.02em",
          }}
        >
          <span>Setup · Configuration · Architecture · Reference</span>
        </div>
      </div>
    </div>,
    { ...size },
  );
}
