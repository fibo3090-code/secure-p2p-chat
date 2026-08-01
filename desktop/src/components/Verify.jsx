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
    // Not dismissable: a live session is waiting on this answer. Escape or a
    // click outside used to clear the prompt without answering, leaving the peer
    // hanging with no explanation and the prompt reappearing on the next poll,
    // which reads as a bug. The only ways out are Verify and Reject.
    <Modal open={!!req} onClose={onClose} width={440} title={`Verify ${req.peer_name}`} icon="fingerprint"
      dismissable={false}
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
        {/* Both outcomes are real choices, so neither is styled as an
            afterthought — rejecting a connection you could not verify is the
            correct action, not an edge case. */}
        <div className="verify-actions">
          <Button icon="shieldCheck" disabled={busy} onClick={() => decide(true)}>
            {busy ? "Working…" : "Codes match — trust"}
          </Button>
          <Button variant="danger" icon="x" disabled={busy} onClick={() => decide(false)}>Reject</Button>
        </div>
        {/* Escape hatch, shown only after a failed decision. The dialog is
            otherwise undismissable so a pending prompt can't be waved away by
            accident — but if the session has gone (the usual reason confirming
            fails) the user must not be trapped in a dialog that can never
            succeed. Nothing is trusted by closing it. */}
        {err && (
          <button className="verify-dismiss" onClick={onClose}>
            The connection seems to be gone — close without trusting
          </button>
        )}
      </div>
    </Modal>
  );
}
