/**
 * Filesystem bridge for the VS Code service layer: a `file://` overlay
 * provider that answers stat/readdir/readFile over Tauri IPC (`ide_stat`,
 * `ide_read_dir`, `ide_read_file`), so workbench features (quick-open file
 * search, editor model resolution) see the real disk. Read-only by design —
 * writes go through CodeViewer's mtime-guarded `ide_write_file` path, never
 * through the provider.
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
  FileSystemProviderCapabilities,
  FileSystemProviderError,
  FileSystemProviderErrorCode,
  FileType,
  registerFileSystemOverlay,
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
    FileSystemProviderCapabilities.PathCaseSensitive |
    FileSystemProviderCapabilities.Readonly;
  onDidChangeCapabilities = Event.None;
  private readonly _onDidChangeFile = this._register(new Emitter<never[]>());
  onDidChangeFile = this._onDidChangeFile.event;

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

  async writeFile(): Promise<void> {
    throw FileSystemProviderError.create(
      "read-only — save through the code viewer",
      FileSystemProviderErrorCode.NoPermissions,
    );
  }

  watch() {
    return Disposable.None;
  }

  async mkdir(): Promise<void> {
    throw FileSystemProviderError.create("read-only", FileSystemProviderErrorCode.NoPermissions);
  }

  async delete(): Promise<void> {
    throw FileSystemProviderError.create("read-only", FileSystemProviderErrorCode.NoPermissions);
  }

  async rename(): Promise<void> {
    throw FileSystemProviderError.create("read-only", FileSystemProviderErrorCode.NoPermissions);
  }
}

/** Overlay the Tauri-backed provider onto `file://`. Call once, after the
 * services initialize. */
export function registerTauriFileSystem(): void {
  registerFileSystemOverlay(1, new TauriFileSystemProvider());
}
