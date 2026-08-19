// @vitest-environment jsdom
//
// Community governance surfaces. Everything here is advisory — the server
// decides and refuses what it disagrees with — so what these tests pin is that
// the UI does not *offer* an action the caller does not hold, and that the
// permission switches mirror the server's normalisation (you cannot grant
// "download" without "see it", or "share on" without "download").

import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  DrivePanel,
  AuditPanel,
  ChannelAccessDialog,
  FilePermissionsDialog,
  ShareFileDialog,
  fmtBytes,
  kindIcon,
  KINDS,
  ROLES,
} from "./PartyAdmin.jsx";

const file = {
  hash: "h1", name: "roadmap.pdf", size: 284134, uploader: "m2", uploader_name: "nova",
  location: "ch-general", location_name: "#general", is_dm: false, shared_at: 1700000000000,
  can_view: true, can_download: true, can_share: true, can_delete: true,
};

const server = {
  id: "s1",
  channels: [
    { id: "ch-general", name: "general", kind: "public", members: [], can_post: true },
    { id: "ch-random", name: "random", kind: "public", members: [], can_post: true },
  ],
  members: [
    { id: "me", username: "you", is_me: true, role: "owner" },
    { id: "m2", username: "nova", is_me: false, role: "member" },
  ],
  files: [file],
  quota: { used: 284134, limit: 134217728, server_used: 284134, server_limit: 1073741824 },
  audit: [{ at: 1700000000000, actor_name: "you", action: "channel.create", detail: "created #random" }],
};

describe("fmtBytes", () => {
  it("scales and keeps one decimal above the byte unit", () => {
    expect(fmtBytes(0)).toBe("0 B");
    expect(fmtBytes(999)).toBe("999 B");
    expect(fmtBytes(1024)).toBe("1.0 KB");
    expect(fmtBytes(9216)).toBe("9.0 KB");
    // One decimal only while it still says something: 277.5 KB rounds.
    expect(fmtBytes(284134)).toBe("277 KB");
    expect(fmtBytes(1073741824)).toBe("1.0 GB");
  });

  it("returns nothing for an absent size rather than 'NaN'", () => {
    expect(fmtBytes(null)).toBe("");
    expect(fmtBytes(undefined)).toBe("");
  });
});

describe("kindIcon", () => {
  it("has an icon for every channel kind the UI offers", () => {
    for (const k of KINDS) expect(kindIcon(k.id)).toBeTruthy();
    expect(kindIcon("something-new")).toBe("hash");
  });
});

describe("ROLES", () => {
  it("never offers 'owner' — the first member to join is the owner and cannot be granted", () => {
    expect(ROLES).not.toContain("owner");
  });
});

describe("DrivePanel", () => {
  it("says the drive is empty rather than showing an empty table", () => {
    render(<DrivePanel server={{ ...server, files: [] }} />);
    expect(screen.getByText("Nothing has been shared here yet.")).toBeTruthy();
  });

  it("lists a file with who shared it, where, and how big", () => {
    render(<DrivePanel server={server} />);
    expect(screen.getByText("roadmap.pdf")).toBeTruthy();
    const sub = document.querySelector(".drive-sub2").textContent;
    expect(sub).toContain("277 KB");
    expect(sub).toContain("nova");
    expect(sub).toContain("#general");
  });

  it("reports both the personal and the server-wide quota", () => {
    render(<DrivePanel server={server} />);
    // A server-wide ceiling alone lets the first member to reach it deny the
    // feature to everyone else, so both numbers are shown.
    expect(document.querySelector(".drive-quota-l").textContent).toBe("277 KB of 128 MB used");
    expect(document.querySelector(".drive-quota-s").textContent).toBe("Server: 277 KB / 1.0 GB");
  });

  it("offers only the actions the member actually holds", () => {
    render(<DrivePanel server={{ ...server, files: [{ ...file, can_download: false, can_share: false, can_delete: false }] }} />);
    expect(screen.queryByTitle(/^Download/)).toBeNull();
    expect(screen.queryByTitle(/^Share/)).toBeNull();
    expect(screen.queryByTitle("Delete this file")).toBeNull();
  });

  it("requires a second click to delete", async () => {
    const onDelete = vi.fn();
    render(<DrivePanel server={server} onDelete={onDelete} onDownload={() => {}} onShare={() => {}} onPermissions={() => {}} />);
    const del = screen.getByTitle("Delete this file");
    await userEvent.click(del);
    expect(onDelete).not.toHaveBeenCalled();

    // Armed: the title now explains what deleting does and does not do.
    const armed = screen.getByTitle(/Click again to delete/);
    expect(armed.getAttribute("title")).toMatch(/stays listed in the conversation/);
    await userEvent.click(armed);
    expect(onDelete).toHaveBeenCalledWith(file);
  });

  it("disables the download button while that file is in flight", () => {
    render(<DrivePanel server={server} downloading={{ h1: true }} onDownload={() => {}} />);
    expect(screen.getByTitle("Download roadmap.pdf").disabled).toBe(true);
  });
});

describe("AuditPanel", () => {
  it("shows what happened and who did it", () => {
    render(<AuditPanel server={server} />);
    expect(screen.getByText(/created #random/)).toBeTruthy();
    expect(screen.getByText(/you/)).toBeTruthy();
  });

  it("handles a server with no audit trail", () => {
    const { container } = render(<AuditPanel server={{ ...server, audit: [] }} />);
    expect(container.textContent.length).toBeGreaterThan(0);
  });
});

describe("ChannelAccessDialog", () => {
  it("will not create a channel with no name", async () => {
    const onSubmit = vi.fn();
    render(<ChannelAccessDialog server={server} channel={null} onClose={() => {}} onSubmit={onSubmit} />);
    expect(screen.getByRole("button", { name: "Create channel" }).disabled).toBe(true);
  });

  it("creates a public channel by default", async () => {
    const onSubmit = vi.fn(async () => {});
    render(<ChannelAccessDialog server={server} channel={null} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.type(screen.getByPlaceholderText("announcements"), "news");
    await userEvent.click(screen.getByRole("button", { name: "Create channel" }));
    expect(onSubmit).toHaveBeenCalledWith({ name: "news", kind: "public", members: [] });
  });

  it("only asks for a member list once the channel is private", async () => {
    render(<ChannelAccessDialog server={server} channel={null} onClose={() => {}} onSubmit={async () => {}} />);
    expect(screen.queryByRole("button", { name: /nova/ })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /Private/ }));
    expect(screen.getByRole("button", { name: /nova/ })).toBeTruthy();
    // The creator is always in it, so they cannot lock themselves out.
    expect(screen.getByText(/cannot lock yourself out/)).toBeTruthy();
  });

  it("carries the picked members through to the submit", async () => {
    const onSubmit = vi.fn(async () => {});
    render(<ChannelAccessDialog server={server} channel={null} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.type(screen.getByPlaceholderText("announcements"), "secret");
    await userEvent.click(screen.getByRole("button", { name: /Private/ }));
    await userEvent.click(screen.getByRole("button", { name: /nova/ }));
    await userEvent.click(screen.getByRole("button", { name: "Create channel" }));
    expect(onSubmit).toHaveBeenCalledWith({ name: "secret", kind: "private", members: ["m2"] });
  });

  it("edits an existing channel without asking for a new name", () => {
    render(
      <ChannelAccessDialog server={server} onClose={() => {}} onSubmit={async () => {}}
        channel={{ id: "ch-general", name: "general", kind: "public", members: [] }} />,
    );
    expect(screen.queryByPlaceholderText("announcements")).toBeNull();
    expect(screen.getByRole("dialog", { name: "Access for #general" })).toBeTruthy();
    expect(screen.getByText(/takes effect immediately for everyone/)).toBeTruthy();
  });
});

describe("FilePermissionsDialog", () => {
  const rights = () => {
    const map = {};
    for (const el of document.querySelectorAll(".perm-opt")) {
      map[el.querySelector(".perm-l").textContent] = el.getAttribute("aria-pressed") === "true";
    }
    return map;
  };

  it("mirrors the server's normalisation: granting download implies seeing it", async () => {
    render(<FilePermissionsDialog server={server} file={{ ...file, can_view: false, can_download: false }} onClose={() => {}} onSubmit={async () => {}} />);
    expect(rights()["See it"]).toBe(false);
    await userEvent.click(screen.getByRole("button", { name: /Download it/ }));
    expect(rights()["Download it"]).toBe(true);
    expect(rights()["See it"]).toBe(true);
  });

  it("revoking 'see it' revokes everything downstream of it", async () => {
    render(<FilePermissionsDialog server={server} file={file} onClose={() => {}} onSubmit={async () => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Share it on/ }));
    expect(rights()["Download it"]).toBe(true);

    await userEvent.click(screen.getByRole("button", { name: /See it/ }));
    expect(rights()).toEqual({
      "See it": false, "Download it": false, "Remove it": false, "Share it on": false,
    });
  });

  it("applies to everyone by default and to one member when picked", async () => {
    const onSubmit = vi.fn(async () => {});
    render(<FilePermissionsDialog server={server} file={file} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: /Everyone here/ }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit.mock.calls[0][0]).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /nova/ }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit.mock.calls[1][0]).toBe("m2");
  });
});

describe("ShareFileDialog", () => {
  it("does not offer the channel the file is already in", () => {
    render(<ShareFileDialog server={server} file={file} onClose={() => {}} onSubmit={async () => {}} />);
    const picks = screen.getAllByRole("button").map((b) => b.textContent);
    expect(picks.some((t) => t.includes("random"))).toBe(true);
    expect(picks.some((t) => t.includes("general"))).toBe(false);
  });

  it("shares into a channel or a DM", async () => {
    const onSubmit = vi.fn(async () => {});
    const { rerender } = render(<ShareFileDialog server={server} file={file} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: /random/ }));
    expect(onSubmit).toHaveBeenCalledWith({ channel: "ch-random" });

    rerender(<ShareFileDialog server={server} file={file} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: /nova/ }));
    expect(onSubmit).toHaveBeenLastCalledWith({ peer: "m2" });
  });

  it("says so when there is nowhere else to put it", () => {
    render(
      <ShareFileDialog file={file} onClose={() => {}} onSubmit={async () => {}}
        server={{ ...server, channels: [server.channels[0]], members: [server.members[0]] }} />,
    );
    expect(screen.getByText("Nowhere else to post it.")).toBeTruthy();
    expect(screen.getByText("Nobody else has joined yet.")).toBeTruthy();
  });

  it("says the re-share does not upload the file again", () => {
    render(<ShareFileDialog server={server} file={file} onClose={() => {}} onSubmit={async () => {}} />);
    // Content-addressed storage: one file shared into three channels costs its
    // size once, and the UI has to say so or the quota reads as a bug.
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/stored once, so this does not upload it again/)).toBeTruthy();
  });
});
