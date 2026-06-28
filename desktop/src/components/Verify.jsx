// Fingerprint (TOFU) verification — colored safety grid + the hex code, with
// accept/reject. Driven by the bridge's `fingerprint-request` event.
import { Modal, Button } from "./ui.jsx";
import { Icon } from "../lib/Icon.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";
import { api } from "../lib/bridge.js";

function group(fp) {
  return (fp || "").replace(/(.{4})/g, "$1 ").trim();
}

export function Verify({ req, onClose }) {
  if (!req) return null;
  async function decide(accept) {
    try { await api.confirmFingerprint(req.chat_id, accept); } catch { /* ignore */ }
    onClose();
  }
  return (
    <Modal open={!!req} onClose={onClose} width={440} title={`Verify ${req.peer_name}`} icon="fingerprint"
      sub="Trust on first use — confirm out of band">
      <div className="verify-body">
        <SafetyGrid fingerprint={req.fingerprint} n={8} cell={28} />
        <div className="verify-code">{group(req.fingerprint)}</div>
        <div className="verify-hint">
          Compare this grid (or the code) with {req.peer_name.split(" ")[0]} over a separate trusted channel —
          a call or in person. Only accept if they match exactly.
        </div>
        <div className="verify-actions">
          <Button icon="shieldCheck" onClick={() => decide(true)}>Verify &amp; trust</Button>
          <Button variant="danger-ghost" icon="x" onClick={() => decide(false)}>Reject</Button>
        </div>
      </div>
    </Modal>
  );
}
