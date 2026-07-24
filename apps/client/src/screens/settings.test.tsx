// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { screen } from "@testing-library/react";
import { SettingsScreen } from "@/screens/settings";
import { renderWithProviders } from "@/test/render";

// Regression guard for the item-17 split: the shell must still compose every
// per-pane module into its tab strip.
describe("SettingsScreen shell", () => {
  it("renders all seven tab triggers", () => {
    renderWithProviders(<SettingsScreen />);
    for (const label of [
      "General",
      "Appearance",
      "Agentboard",
      "Journal",
      "Collectors",
      "Shortcuts",
      "About",
    ]) {
      expect(screen.getByRole("tab", { name: label })).toBeInTheDocument();
    }
  });
});
