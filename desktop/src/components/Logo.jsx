// The P2PEM mark — the same artwork as the app icon
// (src-tauri/app-icon-mark.svg): a teal→indigo chat bubble carrying two linked
// nodes. Inlined rather than loaded as a file so it inherits no network or CSP
// concerns and renders crisply at every size. The node ink follows the theme
// background, so the mark sits on dark and light surfaces alike.
import { useId } from "react";

export function Logo({ size = 32, title }) {
  const gid = `p2pem-bubble-${useId().replace(/:/g, "")}`;
  return (
    <svg className="logo" width={size} height={size} viewBox="190 170 644 660"
      role={title ? "img" : undefined} aria-label={title} aria-hidden={title ? undefined : true}
      focusable="false" xmlns="http://www.w3.org/2000/svg">
      {title && <title>{title}</title>}
      <defs>
        <linearGradient id={gid} x1="0" y1="1" x2="1" y2="0">
          <stop offset="0" stopColor="#2dd4bf" />
          <stop offset="1" stopColor="#4f46e5" />
        </linearGradient>
      </defs>
      <g transform="translate(512 500) scale(1.58)">
        <path d="M-190 -120 a70 70 0 0 1 70 -70 h240 a70 70 0 0 1 70 70 v190 a70 70 0 0 1 -70 70 h-179 l-70 84 a10 10 0 0 1 -17.6 -6.6 l-3.4 -77.6 a70 70 0 0 1 -40 -63.8 Z"
          fill={`url(#${gid})`} />
        <g className="logo-ink">
          <line x1="-64" y1="31" x2="64" y2="-80" strokeWidth="34" strokeLinecap="round" />
          <circle cx="-64" cy="31" r="42" />
          <circle cx="64" cy="-80" r="42" />
        </g>
      </g>
    </svg>
  );
}
