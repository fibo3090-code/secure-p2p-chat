// Fingerprint (TOFU) verification — colored safety grid + the hex code, with
// accept/reject. Driven by the bridge's `fingerprint-request` event.
import { useState, useEffect } from "react";
import { Modal, Button } from "./ui.jsx";
import { Icon } from "../lib/Icon.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";
import { api, friendlyError } from "../lib/bridge.js";
function group(fp) {
  return (fp || "").replace(/(.{4})/g, "$1 ").trim();
}

export function Verify({ req, onClose }) {
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  // Stays mounted, so clear a previous request's error when a new one arrives.
  useEffect(() => { setErr(""); }, [req?.chat_id]);
  if (!req) return null;
  async function decide(accept) {
    if (busy) return;
    setBusy(true);
    setErr("");
    // Dismiss only on success: the backend keeps the prompt queued on failure.
    try {
      await api.confirmFingerprint(req.chat_id, accept);
      onClose();
    } catch (e) {
      setErr(friendlyError(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    // Not dismissable: a live session waits on this answer, so the only ways out
    // are Verify and Reject.
    <Modal open={!!req} onClose={onClose} width={440} title={`Verify ${req.peer_name}`} icon="fingerprint"
      dismissable={false}
      sub="Trust on first use — confirm out of band">
      <div className="verify-body">
        {req.sas ? (
          <>
            {/* The SAS leads: both peers see the same code unless a MITM sits
                between them, and it is easier to read aloud than 64 hex chars. */}
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
        {/* Rejecting an unverifiable connection is a real choice, not an edge case. */}
        <div className="verify-actions">
          <Button icon="shieldCheck" disabled={busy} onClick={() => decide(true)}>
            {busy ? "Working…" : "Codes match — trust"}
          </Button>
          <Button variant="danger" icon="x" disabled={busy} onClick={() => decide(false)}>Reject</Button>
        </div>
        {/* Escape hatch after a failed decision (usually the session is gone),
            so the user is not trapped. Closing trusts nothing. */}
        {err && (
          <button className="verify-dismiss" onClick={onClose}>
            The connection seems to be gone — close without trusting
          </button>
        )}
      </div>
    </Modal>
  );
}
