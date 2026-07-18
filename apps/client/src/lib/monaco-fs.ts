/**
 * Filesystem bridge for the VS Code service layer: a `file://` overlay
 * provider that answers stat/readdir/readFile over Tauri IPC (`ide_stat`,
 * `ide_read_dir`, `ide_read_file`), so workbench features (quick-open file
 * search, editor model resolution) see the real disk, plus the mutations
 * behind the Explorer's New File / New Folder / Rename / Delete.
 *
 * The code viewer does NOT save through here. It keeps its own
 * `ide_write_file` call with an mtime conflict token, which refuses to
 * clobber a file an agent edited while it was open — routing it through the
 * provider would quietly drop that guard.
 *
 * `@codingame/monaco-vscode-files-service-override` is imported here but is
 * deliberately NOT a direct dependency in package.json, and it looks like an
 * oversight every time someone reads it. Declaring it adds it to
 * `monacoVscodeDeps`, which feeds `optimizeDeps.include` in vite.config.ts —
 * and pre-bundling it as its own entry yields a *second* copy of the files
 * service. The overlay below then registers on one instance while the search
 * service walks the other, and quick-open silently reports "No matching
 * results" for every query. Leave it transitive so there's exactly one copy.
 */

import {
  FileChangeType,
  FileSystemProviderCapabilities,
  FileSystemProviderError,
  FileSystemProviderErrorCode,
  FileType,
  registerFileSystemOverlay,
  type IFileChange,
  type IFileDeleteOptions,
  type IFileOverwriteOptions,
  type IFileWriteOptions,
  type IFileSystemProviderWithFileReadWriteCapability,
  type IStat,
} from "@codingame/monaco-vscode-files-service-override";
import { Emitter, Event } from "@codingame/monaco-vscode-api/vscode/vs/base/common/event";
import type { URI } from "@codingame/monaco-vscode-api/vscode/vs/base/common/uri";
import { Disposable } from "@codingame/monaco-vscode-api/vscode/vs/base/common/lifecycle";
import { invoke } from "@/lib/tauri";
import { errorMessage } from "@/lib/errors";

type FsStat = { isDir: boolean; size: number; mtimeMs: number };
type FsDirEntry = { name: string; isDir: boolean };

function notFound(): FileSystemProviderError {
  return FileSystemProviderError.create("file not found", FileSystemProviderErrorCode.FileNotFound);
}

/**
 * Bytes → the string `ide_write_file` takes, refusing anything that isn't
 * valid UTF-8.
 *
 * A non-fatal decode maps every undecodable byte to U+FFFD, so writing a PNG
 * back through here would replace it with mojibake and report success. Since
 * the command's parameter is a Rust `String` there is no lossless path, and
 * `ide_read_file` already refuses to read a file containing NUL — failing
 * loudly on the write keeps the pair symmetric instead of corrupting a file
 * the bridge should never have touched.
 */
function decodeText(content: Uint8Array, filePath: string): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(content);
  } catch {
    throw FileSystemProviderError.create(
      `${filePath} is not valid UTF-8 — the editor bridge only writes text files`,
      FileSystemProviderErrorCode.Unknown,
    );
  }
}

class TauriFileSystemProvider
  extends Disposable
  implements IFileSystemProviderWithFileReadWriteCapability
{
  // No `Readonly` bit — it is not decorative here. `OverlayFileSystemProvider`
  // skips any delegate carrying it in `writeToDelegates`, so every mutation
  // below (writeFile / mkdir / delete / rename) would be passed over and the
  // overlay would throw "Not allowed"; its `stat` also stamps
  // `FilePermission.Readonly` onto every file it returns.
  capabilities =
    FileSystemProviderCapabilities.FileReadWrite | FileSystemProviderCapabilities.PathCaseSensitive;
  onDidChangeCapabilities = Event.None;
  private readonly _onDidChangeFile = this._register(new Emitter<readonly IFileChange[]>());
  onDidChangeFile = this._onDidChangeFile.event;

  /** Nothing watches the disk, so the Explorer only learns about changes this
   * provider made itself — enough to keep the tree honest after its own
   * New File / Rename / Delete, which is what it renders. Variadic so a
   * rename reports its two halves in one pass. */
  private changed(...changes: IFileChange[]): void {
    this._onDidChangeFile.fire(changes);
  }

  /** Stat without turning a miss into a throw — `writeFile` has to tell
   * "missing" from "present" to honor its options, and an exception is the
   * wrong shape for a question. */
  private async statOrNull(filePath: string): Promise<FsStat | null> {
    const stat = await invoke<FsStat>("ide_stat", { dir: "/", filePath });
    return stat.unwrapOr(null);
  }

  async stat(resource: URI): Promise<IStat> {
    const s = await this.statOrNull(resource.path.slice(1));
    if (s == null) throw notFound();
    return {
      type: s.isDir ? FileType.Directory : FileType.File,
      ctime: s.mtimeMs,
      mtime: s.mtimeMs,
      size: s.size,
    };
  }

  async readdir(resource: URI): Promise<[string, FileType][]> {
    const entries = await invoke<FsDirEntry[]>("ide_read_dir", {
      dir: "/",
      filePath: resource.path.slice(1),
    });
    if (entries.isErr()) throw notFound();
    return entries.value.map((e) => [e.name, e.isDir ? FileType.Directory : FileType.File]);
  }

  async readFile(resource: URI): Promise<Uint8Array> {
    const read = await invoke<{ content: string }>("ide_read_file", {
      dir: "/",
      filePath: resource.path.slice(1),
    });
    if (read.isErr()) throw notFound();
    return new TextEncoder().encode(read.value.content);
  }

  /**
   * The workbench's own save path (an Explorer "New File", say). The code
   * viewer does NOT come through here — it saves via `ide_write_file`, whose
   * mtime token refuses to clobber a file an agent edited underneath it.
   *
   * `create`/`overwrite` are enforced here rather than trusted to the caller:
   * `IFileService.create` happens to validate first today, but the provider
   * contract is what the next caller will rely on, and getting it wrong means
   * silently truncating a file nobody asked to replace.
   */
  async writeFile(resource: URI, content: Uint8Array, opts: IFileWriteOptions): Promise<void> {
    const filePath = resource.path.slice(1);
    const text = decodeText(content, filePath);
    if (!opts.create || !opts.overwrite) {
      const existing = await this.statOrNull(filePath);
      if (existing != null && !opts.overwrite) {
        throw FileSystemProviderError.create(
          `${filePath} already exists`,
          FileSystemProviderErrorCode.FileExists,
        );
      }
      if (existing == null && !opts.create) throw notFound();
    }
    await this.run("ide_write_file", {
      dir: "/",
      filePath,
      content: text,
      expectedMtimeMs: null,
    });
    this.changed({
      type: opts.overwrite ? FileChangeType.UPDATED : FileChangeType.ADDED,
      resource,
    });
  }

  watch() {
    return Disposable.None;
  }

  async mkdir(resource: URI): Promise<void> {
    await this.run("ide_create_dir", {
      dir: "/",
      filePath: resource.path.slice(1),
    });
    this.changed({ type: FileChangeType.ADDED, resource });
  }

  /**
   * Always trashes, ignoring `opts.useTrash` — that flag is never true here.
   * `OverlayFileSystemProvider` hardcodes its own capabilities and drops the
   * `Trash` bit, so the file service asks for a permanent delete every time.
   *
   * Registering directly with `registerCustomProvider` DOES surface the
   * capability (verified: `hasCapability(uri, Trash)` becomes true, and
   * shift-delete then differs from Delete) — but it also breaks quick-open,
   * which silently returns "No matching results" for every query. The overlay
   * additionally advertises `FileReadStream | FileAppend`, which this provider
   * doesn't implement and so can't claim, and the workspace search provider
   * needs them. Ctrl+P is worth more than shift-delete, so: keep the overlay,
   * always trash (a checkout is full of untracked files git can't bring back),
   * and correct the confirmation copy in `deleteCopyForTrash`.
   */
  async delete(resource: URI, opts: IFileDeleteOptions): Promise<void> {
    await this.run("ide_delete", {
      dir: "/",
      filePath: resource.path.slice(1),
      recursive: opts.recursive,
      useTrash: true,
    });
    this.changed({ type: FileChangeType.DELETED, resource });
  }

  async rename(from: URI, to: URI, opts: IFileOverwriteOptions): Promise<void> {
    await this.run("ide_rename", {
      dir: "/",
      fromPath: from.path.slice(1),
      toPath: to.path.slice(1),
      overwrite: opts.overwrite,
    });
    this.changed(
      { type: FileChangeType.DELETED, resource: from },
      { type: FileChangeType.ADDED, resource: to },
    );
  }

  /** Surface the Rust error text — these are user-initiated actions, so
   * "already exists" or a permission problem has to reach the user rather
   * than collapsing into a generic failure. */
  private async run(cmd: string, args: Record<string, unknown>): Promise<void> {
    const ran = await invoke(cmd, args);
    if (ran.isErr()) {
      const message = errorMessage(ran.error);
      throw FileSystemProviderError.create(message, errorCodeFor(message));
    }
  }
}

/**
 * The code matters, not just the text: VS Code offers an overwrite prompt on
 * `FileExists` and gives up on `Unknown`. These substrings are the contract
 * with `ide.rs` — they're pinned there by `ERR_ALREADY_EXISTS` /
 * `ERR_ESCAPES_FOLDER` and a Rust test, so a reworded message fails loudly
 * instead of silently downgrading to `Unknown`.
 */
const ERROR_CODES: readonly (readonly [string, FileSystemProviderErrorCode])[] = [
  ["already exists", FileSystemProviderErrorCode.FileExists],
  ["escapes the folder", FileSystemProviderErrorCode.NoPermissions],
];

function errorCodeFor(message: string): FileSystemProviderErrorCode {
  return (
    ERROR_CODES.find(([needle]) => message.includes(needle))?.[1] ??
    FileSystemProviderErrorCode.Unknown
  );
}

/** Overlay the Tauri-backed provider onto `file://`. Call once, after the
 * services initialize. See `delete` above for why this is an overlay rather
 * than a direct `registerCustomProvider`. */
export function registerTauriFileSystem(): void {
  registerFileSystemOverlay(1, new TauriFileSystemProvider());
}
