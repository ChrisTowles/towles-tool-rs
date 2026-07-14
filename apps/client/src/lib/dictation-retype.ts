/**
 * TS port of `crates/tt-dictate/src/retype.rs`'s `RetypeState` — computes the
 * minimum backspaces + insert to take a focused element from the previously-
 * applied transcript text to a new one, then applies it to the DOM.
 *
 * Two apply strategies live here:
 * - {@link RetypeState.applyToElement}: React-controlled `<input>`/`<textarea>`.
 *   React reverts a naive `el.value = x` assignment on the next render, so we
 *   go through the native value setter + a dispatched `input` event, which
 *   React's synthetic event system does observe.
 * - {@link committedDelta}: terminal target. Commit-only, append-only — a
 *   terminal has no cursor-addressable "select and replace", and this engine
 *   must never send a backspace into a live PTY (per the #207 design: "never
 *   backspaces into a PTY"). Only the *growth* of committed segments since the
 *   last flush is returned, for the caller to `term_write`.
 */

/** What {@link RetypeState.diff} computes — backspaces then insert. */
export interface RetypeStep {
  backspaces: number;
  insert: string;
}

export function isNoopStep(step: RetypeStep): boolean {
  return step.backspaces === 0 && step.insert === "";
}

/**
 * Tracks what has been typed into the focused element so each new transcript
 * costs the minimum keystrokes. One instance per recording session.
 */
export class RetypeState {
  private typedText = "";

  get text(): string {
    return this.typedText;
  }

  /** Compute the diff from `typedText` to `newText` and update `typedText`. */
  diff(newText: string): RetypeStep {
    const text = newText.replace(/ +$/, "");

    if (text === "" || text === this.typedText) {
      return { backspaces: 0, insert: "" };
    }

    if (text.startsWith(this.typedText)) {
      const insert = text.slice(this.typedText.length);
      this.typedText = text;
      return { backspaces: 0, insert };
    }

    const commonChars = longestCommonPrefixChars(this.typedText, text);
    const prevChars = [...this.typedText].length;
    const insert = sliceByChars(text, commonChars);
    const backspaces = prevChars - commonChars;

    this.typedText = text;
    return { backspaces, insert };
  }

  /** Apply a diff step to a React-controlled input/textarea. */
  applyToElement(el: HTMLInputElement | HTMLTextAreaElement, newText: string): void {
    const step = this.diff(newText);
    if (isNoopStep(step)) return;
    setReactControlledValue(el, this.typedText);
  }

  reset(): void {
    this.typedText = "";
  }
}

/** Number of Unicode code points shared as a prefix by `a` and `b`. */
function longestCommonPrefixChars(a: string, b: string): number {
  const ac = [...a];
  const bc = [...b];
  let i = 0;
  while (i < ac.length && i < bc.length && ac[i] === bc[i]) i++;
  return i;
}

/** `s` from the `charIndex`-th Unicode code point onward. */
function sliceByChars(s: string, charIndex: number): string {
  return [...s].slice(charIndex).join("");
}

/**
 * Set a React-controlled input/textarea's value via the native setter, then
 * dispatch an `input` event so React's synthetic event system (and any
 * `onChange` handler) observes the change. A plain `el.value = x` is silently
 * reverted by React on its next render because React tracks the value it last
 * rendered, not the DOM's live value.
 */
export function setReactControlledValue(el: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

/**
 * Terminal target: commit-only, append-only. Returns the substring of
 * `committed.join(" ")` that is new since `prevSent`, or `null` if there's
 * nothing new (never backspaces — a shrinking/diverging commit history is
 * simply not re-sent). Call with the full committed-segment list on every
 * transcript event; track `prevSent` yourself and update it to the returned
 * full committed text after a successful `term_write`.
 */
export function committedDelta(prevSent: string, committed: string[]): { delta: string; sent: string } | null {
  const full = committed.join(" ");
  if (full === prevSent) return null;
  if (full.startsWith(prevSent)) {
    return { delta: full.slice(prevSent.length), sent: full };
  }
  // Committed history diverged from what we already sent (rare — an
  // endpoint rewrote something already flushed to the PTY). We cannot
  // un-send characters, so just track the new baseline without emitting.
  return { delta: "", sent: full };
}
