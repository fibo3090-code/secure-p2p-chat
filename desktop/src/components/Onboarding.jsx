// Minimalist auth screens — centered brand, a single clear action, real
// password capture wired to the bridge. Also the app's boot screen: the first
// thing a user ever sees is `BootScreen`, so it has to be able to explain
// itself when the bridge is slow or fails rather than render nothing.
import { Logo } from "./Logo.jsx";
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Button, PasswordInput } from "./ui.jsx";
import { pwStrength, passwordFormError, FALLBACK_MIN_PASSWORD } from "../lib/password.js";
import { friendlyError } from "../lib/bridge.js";

function Brand({ sub }) {
  return (
    <div className="brand">
      <span className="brand-mark"><Logo size={56} /></span>
      <div>
        <div className="brand-name">P2PEM</div>
        <div className="brand-sub">{sub || "Encrypted peer-to-peer messenger"}</div>
      </div>
    </div>
  );
}

/// Shown while the bridge is being reached, and when that failed (never a blank
/// window). `fatal` is a failure retrying cannot fix — an identity file that
/// exists but cannot be read, which the app refuses to replace (that would
/// abandon history and contacts' trust) — so it explains instead of offering a retry.
export function BootScreen({ error, retrying, onRetry, fatal }) {
  if (fatal) {
    return (
      <div className="onb-stage">
        <div className="onb-card">
          <Brand sub="Your identity could not be opened" />
          <div className="onb-p-min">
            P2PEM stopped instead of starting fresh. Creating a new identity here
            would permanently abandon the existing one — your message history
            would become unreadable, and everyone who verified you would see a
            different fingerprint.
          </div>
          <pre className="onb-fatal">{error}</pre>
          <div className="onb-fhint">
            If you have a backup (Settings › Identity backup on another install),
            restore it over the file above and reopen the app.
          </div>
        </div>
        <div className="onb-footer-note">Nothing has been changed or deleted</div>
      </div>
    );
  }
  return (
    <div className="onb-stage">
      <div className="onb-card">
        <Brand sub={error ? "Couldn't start" : "Starting up"} />
        {error ? (
          <>
            <div className="onb-p-min">
              The app couldn't reach its secure backend. Your data is untouched —
              nothing is decrypted until this succeeds.
            </div>
            <code className="onb-fp-code">{error}</code>
            <Button full icon="refresh" onClick={onRetry} disabled={retrying}>
              {retrying ? "Retrying…" : "Try again"}
            </Button>
            <div className="onb-fhint">
              Retrying automatically. If this keeps happening, restart the app — and
              if it still fails, export diagnostics from another install or file an issue.
            </div>
          </>
        ) : (
          <>
            <div className="onb-boot-spinner" aria-hidden="true" />
            <div className="onb-p-min">Unlocking the secure backend…</div>
          </>
        )}
      </div>
      <div className="onb-footer-note">Nothing decrypts until you unlock · keys never leave this device</div>
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
      <form className="onb-card" onSubmit={submit}>
        <Brand />
        <div className="onb-p-min">Enter your password to unlock your identity and history.</div>
        <div className="onb-field">
          <label className="onb-flabel" htmlFor="unlock-pw">Password</label>
          <PasswordInput id="unlock-pw" value={pw} autoFocus disabled={busy}
            autoComplete="current-password"
            aria-invalid={!!err || undefined} aria-describedby={err ? "unlock-err" : undefined}
            onChange={(e) => { setPw(e.target.value); setErr(""); }} />
          <div aria-live="assertive">
            {err && <span className="onb-err" id="unlock-err"><Icon name="alert" size={13} /> {err}</span>}
          </div>
        </div>
        <Button type="submit" full icon="unlock" disabled={busy}>{busy ? "Unlocking…" : "Unlock"}</Button>
      </form>
      <div className="onb-footer-note">Nothing decrypts until you unlock · keys never leave this device</div>
    </div>
  );
}

export function SetPasswordScreen({ minLength, onSet }) {
  const min = minLength || FALLBACK_MIN_PASSWORD;
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const strength = pwStrength(pw, min);
  const mismatch = !!pw2 && pw !== pw2;
  const formError = passwordFormError(pw, pw2, min);

  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    // Say why nothing happened, or a mistyped confirmation looks like a broken button.
    if (formError) { setErr(formError); return; }
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
      <form className="onb-card" onSubmit={submit}>
        <Brand sub="Secure your new identity" />
        <div className="onb-p-min">
          This password encrypts your identity and message history on this device.
        </div>
        {/* The one irreversible fact, said where it cannot be missed. */}
        <div className="onb-warn" role="note">
          <Icon name="alert" size={15} />
          <span><strong>There is no recovery.</strong> Forget this password and your identity and history are gone — nobody, including us, can reset it.</span>
        </div>
        <div className="onb-field">
          <label className="onb-flabel" htmlFor="new-pw">New password</label>
          <PasswordInput id="new-pw" value={pw} autoFocus disabled={busy}
            autoComplete="new-password" aria-describedby="new-pw-hint"
            onChange={(e) => { setPw(e.target.value); setErr(""); }} />
          <div className="onb-strength" id="new-pw-hint">
            {/* Neutral until the first keystroke: a red bar before anything is
                typed reads as an error the user has not made. */}
            <span className={"onb-strength-bar " + (pw ? "s" + strength.score : "s-empty")} />
            <span className="onb-fhint">{pw ? strength.label : `At least ${min} characters`}</span>
          </div>
        </div>
        <div className="onb-field">
          <label className="onb-flabel" htmlFor="new-pw2">Confirm password</label>
          <PasswordInput id="new-pw2" value={pw2} disabled={busy}
            autoComplete="new-password"
            aria-invalid={mismatch || undefined}
            aria-describedby={mismatch ? "new-pw2-err" : undefined}
            onChange={(e) => { setPw2(e.target.value); setErr(""); }} />
          <div aria-live="polite">
            {mismatch && <span className="onb-err" id="new-pw2-err"><Icon name="alert" size={13} /> Passwords don't match</span>}
            {err && <span className="onb-err"><Icon name="alert" size={13} /> {err}</span>}
          </div>
        </div>
        <Button type="submit" full icon="shieldCheck" disabled={busy || !!formError}>
          {busy ? "Securing…" : "Create identity"}
        </Button>
      </form>
      <div className="onb-footer-note">RSA + X25519 keys · generated locally</div>
    </div>
  );
}

/// Shown once, right after an identity is created: losing the file loses the
/// identity outright, so this is the moment to offer the backup.
export function BackupPrompt({ onExport, onSkip }) {
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState("");
  const [err, setErr] = useState("");

  async function exportNow() {
    setBusy(true);
    setErr("");
    try {
      const dest = await onExport();
      if (dest) setDone(dest);
    } catch (e) {
      setErr(friendlyError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onb-stage">
      <div className="onb-card">
        <Brand sub="Back up your identity" />
        <div className="onb-p-min">
          Your identity lives only on this device. If the disk dies you lose the
          key your contacts have verified, and you start over as a stranger to
          everyone. Save a copy somewhere safe now — it takes ten seconds.
        </div>
        <div className="onb-fhint">
          The backup file is already encrypted with the password you just chose,
          so it is exactly as safe as the original. You still need that password
          to use it. It holds your identity only — not your message history.
        </div>
        {done
          ? <code className="onb-fp-code">Saved to {done}</code>
          : err && <span className="onb-err"><Icon name="alert" size={13} /> {err}</span>}
        {done ? (
          <Button full icon="check" onClick={onSkip}>Done — open P2PEM</Button>
        ) : (
          <>
            <Button full icon="copy" onClick={exportNow} disabled={busy}>
              {busy ? "Saving…" : "Save backup file"}
            </Button>
            <button type="button" className="onb-skip" onClick={onSkip}>
              Skip for now — I'll do it from Settings
            </button>
          </>
        )}
      </div>
      <div className="onb-footer-note">Settings › Your identity › Identity backup, any time</div>
    </div>
  );
}
