// @vitest-environment jsdom
//
// The bridge's pure parts: the two rules the dev mock mirrors from Rust, the
// error sanitiser, and the adapters that turn backend payloads into the shapes
// the components render.
//
// The mock is tested as well as the helpers. A dev mock that is *more*
// permissive than the real bridge is worse than none: it makes the security
// paths invisible during browser-based UI work, so the only place they are ever
// exercised is a user's machine.

import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  isOpenableUrl,
  executionRisk,
  friendlyError,
  summaryToConv,
  chatToContact,
  fmtDay,
  api,
} from "./bridge.js";

describe("isOpenableUrl", () => {
  it("allows http and https", () => {
    expect(isOpenableUrl("http://example.com")).toBe(true);
    expect(isOpenableUrl("https://example.com/a?b=c")).toBe(true);
    expect(isOpenableUrl("HTTPS://EXAMPLE.COM")).toBe(true);
    expect(isOpenableUrl("  https://example.com  ")).toBe(true);
  });

  it("refuses the schemes that execute or read local state", () => {
    // `javascript:` and `data:` handed to window.open run in the opener's
    // origin — a message body would be enough to script the app.
    for (const url of [
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "file:///etc/passwd",
      "chat-p2p://invite/abc",
      "vbscript:msgbox(1)",
      "",
      null,
      undefined,
    ]) {
      expect(isOpenableUrl(url)).toBe(false);
    }
  });

  it("is not fooled by a scheme appearing later in the string", () => {
    expect(isOpenableUrl("javascript:void(https://example.com)")).toBe(false);
  });
});

describe("executionRisk", () => {
  it("catches the classic double extension — only the last one counts", () => {
    expect(executionRisk("holiday.jpg.exe")).toBe("exe");
    expect(executionRisk("invoice.pdf.js")).toBe("js");
  });

  it("covers Windows, Unix and cross-platform runtimes", () => {
    for (const [name, ext] of [
      ["setup.msi", "msi"], ["run.bat", "bat"], ["thing.lnk", "lnk"],
      ["script.ps1", "ps1"], ["app.desktop", "desktop"], ["tool.sh", "sh"],
      ["bundle.jar", "jar"], ["thing.appimage", "appimage"], ["pkg.deb", "deb"],
    ]) {
      expect(executionRisk(name)).toBe(ext);
    }
  });

  it("is case-insensitive", () => {
    expect(executionRisk("HOLIDAY.EXE")).toBe("exe");
  });

  it("passes ordinary content through", () => {
    for (const name of ["photo.png", "report.pdf", "notes.txt", "archive.zip", "README", ""]) {
      expect(executionRisk(name)).toBeNull();
    }
  });
});

describe("friendlyError", () => {
  it("keeps the sentence that says what failed", () => {
    expect(friendlyError("Connection refused")).toBe("Connection refused.");
    expect(friendlyError(new Error("invite fingerprint does not match its key")))
      .toBe("invite fingerprint does not match its key.");
  });

  it("strips absolute paths, which are the disclosure — not the explanation", () => {
    // A screenshot or a bug report should not carry the local username and
    // directory layout.
    expect(friendlyError("failed to open /home/maya/.local/share/P2PEM/history.json.enc"))
      .not.toMatch(/maya/);
    expect(friendlyError("failed to open C:\\Users\\Maya\\AppData\\P2PEM\\identity.json"))
      .not.toMatch(/Maya/);
    expect(friendlyError("could not read /Users/maya/Documents/x")).not.toMatch(/maya/);
  });

  it("drops the Error: prefix and source locations", () => {
    expect(friendlyError("Error: send failed")).toBe("send failed.");
    expect(friendlyError("TypeError: bad thing")).toBe("bad thing.");
    expect(friendlyError("send failed (session.rs:412:9)")).toBe("send failed.");
  });

  it("keeps the first cause of an anyhow chain rather than the whole wall", () => {
    expect(friendlyError("Failed to send message: Broken pipe: os error 32"))
      .toBe("Failed to send message.");
  });

  it("falls back rather than showing an empty toast", () => {
    expect(friendlyError("")).toBe("Something went wrong.");
    expect(friendlyError(null)).toBe("Something went wrong.");
    expect(friendlyError("/home/maya/x")).toBe("Something went wrong.");
    expect(friendlyError("", "Could not connect.")).toBe("Could not connect.");
  });

  it("never returns something with a double full stop", () => {
    expect(friendlyError("already ends in a period.")).toBe("already ends in a period.");
  });
});

describe("the dev mock", () => {
  // Outside Tauri, `api` is the mock. These assert the mock enforces the same
  // rules the Rust commands do.
  it("refuses a non-http(s) url exactly as the bridge does", async () => {
    await expect(api.openUrl("javascript:alert(1)")).rejects.toThrow(/http\(s\)/);
  });

  it("opens an http url", async () => {
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    await api.openUrl("https://example.com");
    expect(open).toHaveBeenCalledWith("https://example.com", "_blank", "noopener");
    open.mockRestore();
  });

  it("blocks opening a peer-sent executable until it is confirmed", async () => {
    const [chat] = await api.listConversations();
    const detail = await api.getConversation(chat.id);
    const risky = detail.messages.find((m) => m.content?.filename?.endsWith(".exe"));
    // The mock seeds one deliberately, so the confirmation dialog is reachable
    // in a plain browser.
    expect(risky).toBeTruthy();
    expect(risky.from_me).toBe(false);

    const blocked = await api.openFile(chat.id, risky.id, false, false);
    expect(blocked).toEqual({ opened: false, blocked: "exe", filename: "holiday.jpg.exe" });

    const confirmed = await api.openFile(chat.id, risky.id, false, true);
    expect(confirmed.opened).toBe(true);
  });

  it("never gates revealing, and never gates ordinary content", async () => {
    const [chat] = await api.listConversations();
    const detail = await api.getConversation(chat.id);
    const risky = detail.messages.find((m) => m.content?.filename?.endsWith(".exe"));
    const ordinary = detail.messages.find((m) => m.content?.filename?.endsWith(".pdf"));

    expect((await api.openFile(chat.id, risky.id, true, false)).opened).toBe(true);
    expect((await api.openFile(chat.id, ordinary.id, false, false)).opened).toBe(true);
  });
});

describe("summaryToConv", () => {
  const base = { id: "c1", title: "Alice", last_at: null };

  it("only claims verified once a fingerprint was actually confirmed", () => {
    expect(summaryToConv({ ...base, verified: true }).trust).toBe("verified");
    expect(summaryToConv({ ...base, verified: false }).trust).toBe("unverified");
  });

  it("maps connection state to the three things the row can draw", () => {
    expect(summaryToConv({ ...base, connected: true }).state).toBe("connected");
    expect(summaryToConv({ ...base, placeholder: true }).state).toBe("hosting");
    expect(summaryToConv({ ...base }).state).toBe("offline");
  });

  it("normalises kind and transport whatever casing the backend sent", () => {
    const c = summaryToConv({ ...base, kind: "Group", transport: "Relay" });
    expect(c.kind).toBe("group");
    expect(c.transport).toBe("relay");
    expect(c.relay).toBe(true);
  });

  it("treats a community transport as relayed for the purposes of the badge", () => {
    expect(summaryToConv({ ...base, transport: "server" }).relay).toBe(true);
  });

  it("defaults an absent unread count to zero rather than NaN", () => {
    expect(summaryToConv(base).unread).toBe(0);
  });

  it("says something useful when there is no last message", () => {
    expect(summaryToConv({ ...base, connected: true }).last).toBe("Connected");
    expect(summaryToConv({ ...base, placeholder: true }).last).toBe("Waiting for a peer…");
    expect(summaryToConv(base).last).toBe("");
  });
});

describe("chatToContact", () => {
  const chat = {
    id: "c1", title: "Alice", peer_fingerprint: "ab".repeat(32), is_host_placeholder: false,
    kind: "dm", transport: "direct", messages: [],
  };

  it("returns null for no chat rather than throwing", () => {
    expect(chatToContact(null)).toBeNull();
  });

  it("treats a stored peer fingerprint as completed verification", () => {
    expect(chatToContact(chat, true).trust).toBe("verified");
    expect(chatToContact({ ...chat, peer_fingerprint: null }, true).trust).toBe("unverified");
  });

  it("adapts a text message", () => {
    const [m] = chatToContact({
      ...chat,
      messages: [{ id: "m1", from_me: true, delivered: true, timestamp: 0, content: { type: "text", text: "hi" } }],
    }, true).messages;
    expect(m).toMatchObject({ id: "m1", from: "me", text: "hi", delivered: true });
  });

  it("gates a file card's actions on whether its path was ever recorded", () => {
    const withPath = chatToContact({
      ...chat,
      messages: [{ id: "f1", from_me: false, timestamp: 0, content: { type: "file", filename: "a.png", size: 2048, path: "/dl/a.png" } }],
    }, true).messages[0];
    expect(withPath).toMatchObject({ kind: "file", name: "a.png", hasPath: true, size: "2.0 KB" });

    // Old history has no path; the card must render as plain rather than
    // offering an open that cannot work.
    const without = chatToContact({
      ...chat,
      messages: [{ id: "f2", from_me: false, timestamp: 0, content: { type: "file", filename: "a.png", size: 2048 } }],
    }, true).messages[0];
    expect(without.hasPath).toBe(false);
  });

  it("uses the placeholder flag for the hosting state", () => {
    expect(chatToContact({ ...chat, is_host_placeholder: true }, false).state).toBe("hosting");
    expect(chatToContact(chat, false).state).toBe("offline");
    expect(chatToContact(chat, true).state).toBe("connected");
  });
});

describe("fmtDay", () => {
  it("names today and yesterday rather than printing a date", () => {
    expect(fmtDay(Date.now())).toBe("Today");
    expect(fmtDay(Date.now() - 86400000)).toBe("Yesterday");
  });

  it("returns nothing for an unparseable timestamp", () => {
    expect(fmtDay("not a date")).toBe("");
  });
});
