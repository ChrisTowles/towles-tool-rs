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
import { invokeOrThrow } from "@/lib/tauri";

type FsStat = { isDir: boolean; size: number; mtimeMs: number };
type FsDirEntry = { name: string; isDir: boolean };

function notFound(): FileSystemProviderError {
  return FileSystemProviderError.create("file not found", FileSystemProviderErrorCode.FileNotFound);
}

class TauriFileSystemProvider
  extends Disposable
  implements IFileSystemProviderWithFileReadWriteCapability
{
  capabilities =
    FileSystemProviderCapabilities.FileReadWrite |
    FileSystemProviderCapabilities.PathCaseSensitive;
  onDidChangeCapabilities = Event.None;
  private readonly _onDidChangeFile = this._register(new Emitter<readonly IFileChange[]>());
  onDidChangeFile = this._onDidChangeFile.event;

  /** Nothing watches the disk, so the Explorer only learns about changes this
   * provider made itself — enough to keep the tree honest after its own
   * New File / Rename / Delete, which is what it renders. */
  private changed(type: FileChangeType, resource: URI): void {
    this._onDidChangeFile.fire([{ type, resource }]);
  }

  async stat(resource: URI): Promise<IStat> {
    let s: FsStat;
    try {
      s = await invokeOrThrow<FsStat>("ide_stat", { dir: "/", filePath: resource.path.slice(1) });
    } catch {
      throw notFound();
    }
    return {
      type: s.isDir ? FileType.Directory : FileType.File,
      ctime: s.mtimeMs,
      mtime: s.mtimeMs,
      size: s.size,
    };
  }

  async readdir(resource: URI): Promise<[string, FileType][]> {
    let entries: FsDirEntry[];
    try {
      entries = await invokeOrThrow<FsDirEntry[]>("ide_read_dir", {
        dir: "/",
        filePath: resource.path.slice(1),
      });
    } catch {
      throw notFound();
    }
    return entries.map((e) => [e.name, e.isDir ? FileType.Directory : FileType.File]);
  }

  async readFile(resource: URI): Promise<Uint8Array> {
    try {
      const read = await invokeOrThrow<{ content: string }>("ide_read_file", {
        dir: "/",
        filePath: resource.path.slice(1),
      });
      return new TextEncoder().encode(read.content);
    } catch {
      throw notFound();
    }
  }

  /**
   * The workbench's own save path (an Explorer "New File", say). The code
   * viewer does NOT come through here — it saves via `ide_write_file`, whose
   * mtime token refuses to clobber a file an agent edited underneath it.
   */
  async writeFile(resource: URI, content: Uint8Array, opts: IFileWriteOptions): Promise<void> {
    const existed = opts.overwrite;
    await this.run("ide_write_file", {
      dir: "/",
      filePath: resource.path.slice(1),
      content: new TextDecoder().decode(content),
      expectedMtimeMs: null,
    });
    this.changed(existed ? FileChangeType.UPDATED : FileChangeType.ADDED, resource);
  }

  watch() {
    return Disposable.None;
  }

  async mkdir(resource: URI): Promise<void> {
    await this.run("ide_create_dir", {
      dir: "/",
      filePath: resource.path.slice(1),
    });
    this.changed(FileChangeType.ADDED, resource);
  }

  /**
   * Always trashes, ignoring `opts.useTrash`. That flag is never true here:
   * `registerFileSystemOverlay` puts this behind `OverlayFileSystemProvider`,
   * which hardcodes its own capabilities (FileReadWrite | PathCaseSensitive |
   * FileReadStream | FileAppend) and drops the `Trash` bit this provider
   * advertises, so the file service believes trashing is unsupported and asks
   * for a permanent delete every time. A checkout is full of untracked files
   * git cannot bring back, so we keep the recoverable behavior and correct the
   * confirmation copy instead — see `deleteCopyForTrash` in `monaco-dialogs`.
   */
  async delete(resource: URI, opts: IFileDeleteOptions): Promise<void> {
    await this.run("ide_delete", {
      dir: "/",
      filePath: resource.path.slice(1),
      recursive: opts.recursive,
      useTrash: true,
    });
    this.changed(FileChangeType.DELETED, resource);
  }

  async rename(from: URI, to: URI, opts: IFileOverwriteOptions): Promise<void> {
    await this.run("ide_rename", {
      dir: "/",
      fromPath: from.path.slice(1),
      toPath: to.path.slice(1),
      overwrite: opts.overwrite,
    });
    this._onDidChangeFile.fire([
      { type: FileChangeType.DELETED, resource: from },
      { type: FileChangeType.ADDED, resource: to },
    ]);
  }

  /** Surface the Rust error text — these are user-initiated actions, so
   * "already exists" or a permission problem has to reach the user rather
   * than collapsing into a generic failure. */
  private async run(cmd: string, args: Record<string, unknown>): Promise<void> {
    try {
      await invokeOrThrow(cmd, args);
    } catch (e) {
      const message = String(e);
      throw FileSystemProviderError.create(
        message,
        message.includes("already exists")
          ? FileSystemProviderErrorCode.FileExists
          : message.includes("escapes the folder")
            ? FileSystemProviderErrorCode.NoPermissions
            : FileSystemProviderErrorCode.Unknown,
      );
    }
  }
}

/** Overlay the Tauri-backed provider onto `file://`. Call once, after the
 * services initialize. */
export function registerTauriFileSystem(): void {
  registerFileSystemOverlay(1, new TauriFileSystemProvider());
}
