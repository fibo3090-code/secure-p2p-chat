// Shown when the history file on disk exists but could not be read.
//
// The backend refuses to save while this is the case (saving would replace the
// unreadable file — every conversation, contact and trust decision — with the
// empty state the failed load left behind). This banner is how the user finds
// out, and the one place they can choose to start over: the old file is moved
// aside under a new name, never deleted.
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { friendlyError } from "../lib/bridge.js";

export function HistoryBanner({ issue, onSetAside }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [confirming, setConfirming] = useState(false);

  async function setAside() {
    setBusy(true);
    setErr("");
    try {
      await onSetAside();
    } catch (e) {
      setErr(friendlyError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="history-banner" role="alert">
      <Icon name="alert" size={16} />
      <div className="history-banner-body">
        <strong>Your message history could not be read.</strong>{" "}
        It has not been changed, and nothing is being saved over it — new
        messages from this session will not be kept until you decide.
        {" "}This usually means the file was written by a newer version of the
        app, or it was not fully downloaded from a cloud folder. Installing the
        newer version again, or making the file available offline, will bring
        it back.
        <div className="history-banner-detail">
          <code>{issue.path}</code> — {issue.error}
        </div>
        {err && <div className="history-banner-err">{err}</div>}
      </div>
      {confirming ? (
        <div className="history-banner-actions">
          <span>Start a new, empty history? The unreadable file is kept, renamed.</span>
          <button className="tb-btn" disabled={busy} onClick={setAside}>
            {busy ? "Moving…" : "Yes, start new"}
          </button>
          <button className="tb-btn" disabled={busy} onClick={() => setConfirming(false)}>Cancel</button>
        </div>
      ) : (
        <div className="history-banner-actions">
          <button className="tb-btn" onClick={() => setConfirming(true)}>
            Start a new history…
          </button>
        </div>
      )}
    </div>
  );
}
