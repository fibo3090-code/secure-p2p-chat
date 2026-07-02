// Minimalist auth screens — centered brand, a single clear action, real
// password capture wired to the bridge.
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Button, PasswordInput } from "./ui.jsx";

function Brand({ sub }) {
  return (
    <div className="brand">
      <span className="brand-mark"><Icon name="shieldCheck" size={26} /></span>
      <div>
        <div className="brand-name">P2PEM</div>
        <div className="brand-sub">{sub || "Encrypted peer-to-peer messenger"}</div>
      </div>
    </div>
  );
}

export function LockScreen({ onUnlock }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    if (!pw) { setErr("Enter your password"); return; }
    setBusy(true);
    try {
      const error = await onUnlock(pw);
      if (error) { setErr(error); setPw(""); }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onb-stage">
      <div className="onb-hex" />
      <form className="onb-card" onSubmit={submit}>
        <Brand />
        <div className="onb-p-min">Enter your password to unlock your identity and history.</div>
        <div className="onb-field">
          <PasswordInput value={pw} autoFocus placeholder="Password" disabled={busy}
            onChange={(e) => { setPw(e.target.value); setErr(""); }} />
          {err && <span className="onb-err"><Icon name="alert" size={13} /> {err}</span>}
        </div>
        <Button type="submit" full icon="unlock" disabled={busy}>{busy ? "Unlocking…" : "Unlock"}</Button>
      </form>
      <div className="onb-footer-note">Nothing decrypts until you unlock · keys never leave this device</div>
    </div>
  );
}

function pwStrength(pw) {
  let s = 0;
  if (pw.length >= 6) s++;
  if (pw.length >= 12) s++;
  if (/[A-Z]/.test(pw) && /[0-9]/.test(pw)) s++;
  if (/[^A-Za-z0-9]/.test(pw)) s++;
  const labels = ["Too short", "Weak", "Okay", "Strong", "Excellent"];
  return { score: Math.min(s, 4), label: pw ? labels[Math.min(s, 4)] : "Use 12+ characters" };
}

export function SetPasswordScreen({ fingerprint, onSet }) {
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const strength = pwStrength(pw);

  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    if (pw.length < 4 || pw !== pw2) return;
    setBusy(true);
    try {
      const error = await onSet(pw);
      if (error) setErr(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onb-stage">
      <div className="onb-hex" />
      <form className="onb-card" onSubmit={submit}>
        <Brand sub="Secure your new identity" />
        <div className="onb-p-min">Set a password to encrypt your identity &amp; history at rest. It can't be reset.</div>
        {fingerprint && <code className="onb-fp-code">{fingerprint.slice(0, 32)}…</code>}
        <div className="onb-field">
          <PasswordInput value={pw} autoFocus placeholder="New password" disabled={busy}
            onChange={(e) => { setPw(e.target.value); setErr(""); }} />
          <div className="onb-strength">
            <span className={"onb-strength-bar s" + strength.score} />
            <span className="onb-fhint">{strength.label}</span>
          </div>
        </div>
        <div className="onb-field">
          <PasswordInput value={pw2} placeholder="Confirm password" disabled={busy} onChange={(e) => setPw2(e.target.value)} />
          {pw2 && pw !== pw2 && <span className="onb-err"><Icon name="alert" size={13} /> Passwords don't match</span>}
          {err && <span className="onb-err"><Icon name="alert" size={13} /> {err}</span>}
        </div>
        <Button type="submit" full icon="shieldCheck" disabled={busy || pw.length < 4 || pw !== pw2}>
          {busy ? "Securing…" : "Create identity"}
        </Button>
      </form>
      <div className="onb-footer-note">RSA + X25519 keys · generated locally</div>
    </div>
  );
}
