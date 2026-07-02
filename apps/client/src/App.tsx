import { useState } from "react";

// Tauri injects a global when running inside the desktop shell. Guard the
// import so the page also works in a plain browser (`npm run dev` without Tauri).
const isTauri = "__TAURI_INTERNALS__" in window;

async function callGreet(name: string): Promise<string> {
  if (!isTauri) {
    return `Hello, ${name}! (running in a plain browser — the Rust greet command is unavailable)`;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("greet", { name });
}

export function App() {
  const [greeting, setGreeting] = useState<string>("");

  return (
    <main
      style={{
        minHeight: "100vh",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: "1.5rem",
        fontFamily:
          "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        color: "#e6e6ef",
        background: "#1b1a29",
      }}
    >
      <h1 style={{ margin: 0, fontSize: "2.5rem" }}>Towles Tool</h1>
      <p style={{ margin: 0, opacity: 0.7 }}>Tauri 2 + React desktop shell</p>
      <button
        onClick={() => callGreet("Chris").then(setGreeting)}
        style={{
          padding: "0.6rem 1.4rem",
          fontSize: "1rem",
          borderRadius: "8px",
          border: "1px solid #3a3856",
          background: "#2a2840",
          color: "#e6e6ef",
          cursor: "pointer",
        }}
      >
        Greet from Rust
      </button>
      {greeting && (
        <p style={{ margin: 0, minHeight: "1.5rem" }}>{greeting}</p>
      )}
    </main>
  );
}
