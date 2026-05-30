/* eslint-disable @next/next/no-img-element */
import { ImageResponse } from "next/og";

export const runtime = "edge";
export const alt = "NativeLink — Remote build execution & caching, engineered for scale";
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
      {/* Brand-purple radial backdrop */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "radial-gradient(ellipse 900px 600px at 85% -10%, rgba(156,124,255,0.32), transparent 60%), radial-gradient(ellipse 600px 400px at 0% 100%, rgba(156,124,255,0.18), transparent 60%)",
        }}
      />

      {/* Subtle dot grid */}
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

      {/* Brand mark — geometric N in a rounded square */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 16,
          position: "relative",
          zIndex: 1,
        }}
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
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path
              d="M5.6 6.5 H8.3 V13.8 L15.2 6.5 H18.4 V17.5 H15.7 V10.2 L8.8 17.5 H5.6 Z"
              fill="#0A090E"
            />
          </svg>
        </div>
        <div
          style={{
            fontSize: 32,
            fontWeight: 600,
            letterSpacing: "-0.02em",
            color: "#F5F4F8",
          }}
        >
          nativelink
        </div>
      </div>

      {/* Headline */}
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
            fontSize: 86,
            fontWeight: 600,
            letterSpacing: "-0.04em",
            lineHeight: 1.05,
            maxWidth: 1000,
          }}
        >
          Builds that finish
          <br />
          <span
            style={{
              background: "linear-gradient(90deg, #9C7CFF 0%, #BCA2FF 100%)",
              backgroundClip: "text",
              WebkitBackgroundClip: "text",
              color: "transparent",
            }}
          >
            before you can blink.
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
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <div style={{ width: 8, height: 8, borderRadius: 999, background: "#58DCB2" }} />
            <span>1B+ build requests / month</span>
          </div>
          <span style={{ color: "#3C3A4E" }}>·</span>
          <span>Rust · Source available · FSL-Apache</span>
        </div>
      </div>
    </div>,
    { ...size },
  );
}
