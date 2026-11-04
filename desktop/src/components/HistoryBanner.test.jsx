// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HistoryBanner } from "./HistoryBanner.jsx";

const issue = { error: "Unsupported history version: 9.0", path: "/data/history.json.enc" };

describe("HistoryBanner", () => {
  it("says saving is blocked and names the file", () => {
    render(<HistoryBanner issue={issue} onSetAside={() => {}} />);
    expect(screen.getByRole("alert").textContent).toMatch(/could not be read/);
    expect(screen.getByRole("alert").textContent).toMatch(/nothing is being saved over it/);
    expect(screen.getByText("/data/history.json.enc")).toBeTruthy();
  });

  it("asks for confirmation before starting over", async () => {
    const onSetAside = vi.fn(async () => {});
    render(<HistoryBanner issue={issue} onSetAside={onSetAside} />);
    await userEvent.click(screen.getByRole("button", { name: /Start a new history/ }));
    expect(onSetAside).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /Yes, start new/ }));
    expect(onSetAside).toHaveBeenCalledTimes(1);
  });
});
