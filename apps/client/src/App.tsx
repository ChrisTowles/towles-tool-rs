import { useState } from "react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

// Tauri injects `__TAURI_INTERNALS__` on window inside the desktop shell.
const isTauri = "__TAURI_INTERNALS__" in window;

/**
 * Pipeline-proof shell: one screen exercising Tailwind utilities, shadcn's
 * Button/Card/Dialog, the Radix portal, and the light/dark toggle. Replace
 * with real screens as they're designed.
 */
export function App() {
  const [count, setCount] = useState(0);

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="flex items-center justify-between border-b px-6 py-3">
        <h1 className="font-heading text-lg font-semibold">Towles Tool</h1>
        <ThemeToggle />
      </header>

      <main className="mx-auto flex max-w-xl flex-col gap-6 p-6">
        <Card>
          <CardHeader>
            <CardTitle>Environment</CardTitle>
            <CardDescription>
              Running in {isTauri ? "the Tauri desktop shell" : "a bare browser"}.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex items-center gap-4">
            <Button onClick={() => setCount((c) => c + 1)}>Clicked {count} times</Button>

            <Dialog>
              <DialogTrigger asChild>
                <Button variant="outline">Open dialog</Button>
              </DialogTrigger>
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>It works</DialogTitle>
                  <DialogDescription>
                    Radix portal, focus trap, and Escape/overlay dismissal — all inside the{" "}
                    {isTauri ? "Tauri WebView" : "browser"}.
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter showCloseButton />
              </DialogContent>
            </Dialog>
          </CardContent>
        </Card>
      </main>
    </div>
  );
}
