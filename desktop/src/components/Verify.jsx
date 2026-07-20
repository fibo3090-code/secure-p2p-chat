// Fingerprint (TOFU) verification — colored safety grid + the hex code, with
// accept/reject. Driven by the bridge's `fingerprint-request` event.
import { useState, useEffect } from "react";
import { Modal, Button } from "./ui.jsx";
import { Icon } from "../lib/Icon.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";
import { api } from "../lib/bridge.js";

function group(fp) {
  return (fp || "").replace(/(.{4})/g, "$1 ").trim();
}

export function Verify({ req, onClose }) {
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  // The component stays mounted (returns null when req is falsy), so clear any
  // stale error when a new request arrives — a security-sensitive dialog must
  // not show a previous request's failure before the user acts.
  useEffect(() => { setErr(""); }, [req?.chat_id]);
  if (!req) return null;
  async function decide(accept) {
    if (busy) return;
    setBusy(true);
    setErr("");
    // Only dismiss on success. The Rust side clears the pending prompt only when
    // the command succeeds, so on failure we keep the modal open to retry.
    try {
      await api.confirmFingerprint(req.chat_id, accept);
      onClose();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal open={!!req} onClose={onClose} width={440} title={`Verify ${req.peer_name}`} icon="fingerprint"
      sub="Trust on first use — confirm out of band">
      <div className="verify-body">
        {req.sas ? (
          <>
            {/* Primary check: the short authentication string. Both peers see
                the SAME code; an interposed MITM makes the two ends differ.
                Reading it aloud is far less error-prone than a 64-char hex
                compare, so it leads and the grid/hex become the backstop. */}
            <div className="verify-sas-label">Read this aloud with {req.peer_name.split(" ")[0]}:</div>
            <div className="verify-sas">{req.sas}</div>
            <div className="verify-hint">
              It must match on both screens. If the codes differ, someone may be
              intercepting the connection — reject it.
            </div>
            <details className="verify-advanced">
              <summary>Full fingerprint (advanced)</summary>
              <SafetyGrid fingerprint={req.fingerprint} n={8} cell={28} />
              <div className="verify-code">{group(req.fingerprint)}</div>
            </details>
          </>
        ) : (
          <>
            <SafetyGrid fingerprint={req.fingerprint} n={8} cell={28} />
            <div className="verify-code">{group(req.fingerprint)}</div>
            <div className="verify-hint">
              Compare this grid (or the code) with {req.peer_name.split(" ")[0]} over a separate trusted channel —
              a call or in person. Only accept if they match exactly.
            </div>
          </>
        )}
        {err && <div className="onb-err"><Icon name="alert" size={13} /> {err}</div>}
        <div className="verify-actions">
          <Button icon="shieldCheck" disabled={busy} onClick={() => decide(true)}>Verify &amp; trust</Button>
          <Button variant="danger-ghost" icon="x" disabled={busy} onClick={() => decide(false)}>Reject</Button>
        </div>
      </div>
    </Modal>
  );
}
