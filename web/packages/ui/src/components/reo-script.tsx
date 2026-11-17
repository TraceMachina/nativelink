/** Reo (reo.dev) visitor identification. Shared by the marketing site and the
 * docs so both report under the same client ID. The snippet is Reo's own
 * loader: it appends a deferred <script> for reo.js and calls Reo.init once
 * that has loaded. */
const REO_CLIENT_ID = "f0c1cb3a41172ab";

const reoInitScript = `!function(){var e,t,n;e="${REO_CLIENT_ID}",t=function(){Reo.init({clientID:"${REO_CLIENT_ID}", enableThirdPartyTracking: true})},(n=document.createElement("script")).src="https://static.reo.dev/"+e+"/reo.js",n.defer=!0,n.onload=t,document.head.appendChild(n)}();`;

/** Render inside <head>. */
export function ReoScript() {
  // biome-ignore lint/security/noDangerouslySetInnerHtml: Vendor-supplied loader with a constant body; nothing user-controlled is interpolated.
  return <script dangerouslySetInnerHTML={{ __html: reoInitScript }} />;
}
