// Communities (Party servers) — administered, multi-channel, persistent rooms
// served by the `messenger-server` crate. The backend exists (server/ +
// client/src/app/party_manager.rs) but is NOT yet exposed through the Tauri
// bridge, so rather than fake a working server UI (dead Join/Verify/Sync
// buttons over hardcoded data), this pane is an honest "not wired yet" state.
// Wire real party commands into desktop/src-tauri/src/lib.rs to bring it live.
import { Icon } from "../lib/Icon.jsx";

export function Parties() {
  return (
    <div className="chat-pane chat-empty">
      <div className="chat-empty-inner">
        <span className="chat-empty-ic"><Icon name="users" size={28} /></span>
        <div className="chat-empty-h">Communities are coming to the desktop app</div>
        <div className="chat-empty-p">
          Party servers — administered, multi-channel rooms that keep history — run today
          through the classic client (<code>cargo run</code>) and the <code>messenger-server</code> crate.
          They aren't wired into this desktop UI yet; this surface will light up once the
          party commands are exposed through the bridge.
        </div>
      </div>
    </div>
  );
}
