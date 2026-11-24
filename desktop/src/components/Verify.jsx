// Fingerprint (TOFU) verification — colored safety grid + the hex code, with
// accept/reject. Driven by the bridge's `fingerprint-request` event.
import { useState, useEffect, useRef } from "react";
import { Modal, Button } from "./ui.jsx";
import { Icon } from "../lib/Icon.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";
import { api, friendlyError, normalizeTransport } from "../lib/bridge.js";
function group(fp) {
  return (fp || "").replace(/(.{4})/g, "$1 ").trim();
}

// The safety code, laid out for reading aloud: digits in two groups, emoji
// apart. A screen reader gets the digits one by one rather than "four hundred
// and eighteen", which is not how two people compare a code.
function SasCode({ sas }) {
  const [digits, emoji] = splitSas(sas);
  const spokenDigits = digits.replace(/\s/g, "").split("").join(" ");
  const spoken = emoji ? `${spokenDigits}, then ${emoji}` : spokenDigits;
  return (
    <div className="verify-sas" role="img" aria-label={`Safety code: ${spoken}`}>
      <span className="verify-sas-digits" aria-hidden="true">{digits}</span>
      {emoji && <span className="verify-sas-emoji" aria-hidden="true">{emoji}</span>}
    </div>
  );
}

// Digits (and the spaces between them) first, emoji after. A single linear
// scan rather than a backtracking pattern.
function splitSas(sas) {
  const s = String(sas || "");
  let i = 0;
  while (i < s.length && /[\d\s]/.test(s[i])) i++;
  return i === 0 ? [s, ""] : [s.slice(0, i).trim(), s.slice(i).trim()];
}

// Fingerprints compared as the same 64 hex characters, whatever spacing or
// case either side used.
const normFp = (fp) => String(fp || "").replace(/\s+/g, "").toLowerCase();

const LOOKUP_TIMEOUT_MS = 400;

export function Verify({ req, onClose }) {
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  // The component stays mounted (returns null when req is falsy), so clear any
  // stale error when a new request arrives — a security-sensitive dialog must
  // not show a previous request's failure before the user acts.
  useEffect(() => { setErr(""); }, [req?.chat_id]);
  // What we already know about this conversation: a fingerprint stored from an
  // earlier verification (so a *different* one now means the key changed), and
  // how the bytes are travelling. A key change must not read as "first
  // connection" — it is the prompt where the user most needs to slow down.
  const [known, setKnown] = useState(null);
  // Until the lookup settles, "trust" stays disabled: a changed-key warning
  // must never appear under a cursor already on the button. Bounded, so a hung
  // call cannot trap the user in an unanswerable prompt.
  const [settled, setSettled] = useState(false);
  const rejectRef = useRef(null);
  useEffect(() => {
    setKnown(null);
    setSettled(false);
    if (!req?.chat_id) { setSettled(true); return undefined; }
    let live = true;
    const done = () => live && setSettled(true);
    const timer = setTimeout(done, LOOKUP_TIMEOUT_MS);
    api.getConversation(req.chat_id)
      .then((chat) => live && chat && setKnown({
        fingerprint: chat.peer_fingerprint || null,
        // The Rust enum serialises as "Direct"/"Relay"; compare normalised.
        transport: chat.transport ? normalizeTransport(chat.transport) : null,
      }))
      .catch(() => {})
      .finally(done);
    return () => { live = false; clearTimeout(timer); };
  }, [req?.chat_id]);
  const changed = !!(known?.fingerprint && req?.fingerprint &&
    normFp(known.fingerprint) !== normFp(req.fingerprint));
  // The lookup resolves after the dialog opened, so focus moves when the
  // changed-key state is known, not only on mount.
  useEffect(() => { if (changed) rejectRef.current?.focus(); }, [changed]);
  if (!req) return null;
  const first = req.peer_name.split(" ")[0];
  const route = known?.transport === "relay" ? "Via relay — the relay can't read this conversation"
    : known?.transport === "server" ? "Via a community server"
      : known?.transport === "direct" ? "Direct connection" : null;
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
      setErr(friendlyError(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    // Not dismissable: a live session is waiting on this answer. Escape or a
    // click outside used to clear the prompt without answering, leaving the peer
    // hanging with no explanation and the prompt reappearing on the next poll,
    // which reads as a bug. The only ways out are Verify and Reject.
    <Modal open={!!req} onClose={onClose} width={440}
      title={changed ? `${first}'s safety code changed` : `Verify ${req.peer_name}`}
      icon={changed ? "alert" : "fingerprint"} tone={changed ? "warn" : undefined}
      dismissable={false}
      sub={changed ? "This could be a new device — or someone intercepting" : `Check it's really ${first}`}>
      <div className="verify-body">
        {changed && (
          <div className="verify-changed" role="alert">
            You verified {first} before, and this is a different key. Only trust it if
            {" "}{first} tells you — by voice or in person — that they reinstalled or changed devices.
          </div>
        )}
        {/* The route reassures; on a changed key it is the wrong moment to. */}
        {route && !changed && <div className="verify-route">{route}</div>}
        {req.sas ? (
          <>
            {/* Primary check: the short authentication string. Both peers see
                the SAME code; an interposed MITM makes the two ends differ.
                Reading it aloud is far less error-prone than a 64-char hex
                compare, so it leads and the grid/hex become the backstop. */}
            <div className="verify-sas-label">Read this aloud with {req.peer_name.split(" ")[0]}:</div>
            <SasCode sas={req.sas} />
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
        {/* Both outcomes are real choices, so neither is an afterthought —
            rejecting a connection you could not verify is the correct action,
            not an edge case. Reject is the tinted danger button rather than a
            solid red one: two full-strength buttons side by side read as an
            alarm, and nothing here is wrong yet. */}
        <div className="verify-actions">
          {/* On a changed key, trusting must be deliberate: equal weight, and
              the default focus on Reject so Enter never accepts out of habit. */}
          <Button ref={rejectRef} variant="danger-ghost" icon="x" disabled={busy}
            onClick={() => decide(false)}>Reject</Button>
          <Button variant={changed ? "ghost" : "primary"} icon="shieldCheck" disabled={busy || !settled}
            onClick={() => decide(true)}>
            {busy ? "Working…" : "Codes match — trust"}
          </Button>
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
