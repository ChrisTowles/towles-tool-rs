/**
 * Pending confirmations raised by the VS Code layer, and the promises they
 * answer.
 *
 * Deliberately free of any `@codingame/*` import. `<MonacoDialogHost>` is
 * mounted at the app root, so anything this module pulls in lands in the entry
 * chunk — importing the service (`monaco-dialogs.ts`) from here would drag the
 * whole monaco-vscode-api graph into app startup, which the lazy `loadMonaco`
 * exists to avoid.
 *
 * Same `get`/`subscribe` shape as `lib/focus-target.ts`, consumed via
 * `useSyncExternalStore`.
 */

export type DialogRequest = {
  id: number;
  message: string;
  detail?: string;
  /** Label for the confirming button, mnemonics already stripped. */
  primary: string;
  danger: boolean;
  resolve: (confirmed: boolean) => void;
};

class DialogStore {
  private pending: readonly DialogRequest[] = [];
  private listeners = new Set<() => void>();
  private seq = 0;

  /** Snapshot for `useSyncExternalStore` — stable reference until it changes. */
  get = (): readonly DialogRequest[] => this.pending;

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };

  /** Queue a question and resolve once the UI answers it. */
  ask(req: Omit<DialogRequest, "id" | "resolve">): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      this.pending = [...this.pending, { ...req, id: ++this.seq, resolve }];
      this.emit();
    });
  }

  /** Answer a request and drop it. Answering twice is a no-op — the host
   * unmounts on the first one, but a stray Escape shouldn't throw. */
  answer(id: number, confirmed: boolean): void {
    const req = this.pending.find((r) => r.id === id);
    if (!req) return;
    this.pending = this.pending.filter((r) => r.id !== id);
    this.emit();
    req.resolve(confirmed);
  }

  private emit(): void {
    for (const fn of [...this.listeners]) fn();
  }
}

export const dialogStore = new DialogStore();
