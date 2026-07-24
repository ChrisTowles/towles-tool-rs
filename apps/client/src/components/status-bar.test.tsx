// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { screen } from "@testing-library/react";
import { StatusBar } from "@/components/status-bar";
import { renderWithProviders } from "@/test/render";

describe("StatusBar", () => {
  it("shows the Doctor shortcut, the browser indicator, and the version", () => {
    renderWithProviders(<StatusBar />);
    expect(screen.getByRole("button", { name: /Doctor/ })).toBeInTheDocument();
    // Outside Tauri the shell label reads "browser", and the version falls
    // back to the dev version.
    expect(screen.getByText("browser")).toBeInTheDocument();
    expect(screen.getByText("tt v0.1.0")).toBeInTheDocument();
  });
});
