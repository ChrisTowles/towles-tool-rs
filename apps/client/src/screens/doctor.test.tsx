// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { screen } from "@testing-library/react";
import { DoctorScreen } from "@/screens/doctor";
import { renderWithProviders } from "@/test/render";

describe("DoctorScreen", () => {
  it("renders the heading and the no-host fallback outside Tauri", async () => {
    renderWithProviders(<DoctorScreen />);
    expect(screen.getByRole("heading", { name: "Doctor" })).toBeInTheDocument();
    // `doctor_run` resolves to NotInTauri, so `report` stays null and the
    // probing state gives way to the browser-dev message.
    expect(await screen.findByText("Not available outside the app.")).toBeInTheDocument();
  });
});
