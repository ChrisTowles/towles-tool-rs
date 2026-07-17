import {
  BookOpenText,
  Database,
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileCog,
  FileImage,
  FileJson,
  FileKey,
  FileLock,
  FileSpreadsheet,
  FileTerminal,
  FileText,
  FileType,
  FileVideo,
  FlaskConical,
  Folder,
  FolderCode,
  FolderGit2,
  FolderOpen,
  GitBranch,
  Package,
  Palette,
  ScrollText,
  type LucideIcon,
} from "lucide-react";

/**
 * File-type icons + folder colors for the Agentboard file trees (diff rail +
 * files pane). Net-new mapping — plannotator (the trees' visual ancestor)
 * renders one generic glyph for every file; its hue budget is spent on git
 * change-status, which `ChangeTypeLetter` already carries here. So file-type
 * hue stays *quiet*: 600/400 light/dark pairs of the language's canonical
 * color, never the app's two meaning-bearing accents on their own terms —
 * violet appears only on Claude-owned names (CLAUDE.md, .claude/) where
 * "agent-ness" is literally what it means, and amber is never used (needs-you
 * stays unambiguous).
 */
export type FileIconSpec = { Icon: LucideIcon; className: string };

const DIM = "text-muted-foreground/50";
const QUIET = "text-muted-foreground/70";
const SKY = "text-sky-600 dark:text-sky-400";
const BLUE = "text-blue-600 dark:text-blue-400";
const CYAN = "text-cyan-600 dark:text-cyan-400";
const TEAL = "text-teal-600 dark:text-teal-400";
const EMERALD = "text-emerald-600 dark:text-emerald-400";
const YELLOW = "text-yellow-600 dark:text-yellow-400";
const ORANGE = "text-orange-600 dark:text-orange-400";
const RED = "text-red-600 dark:text-red-400";
const PINK = "text-pink-600 dark:text-pink-400";
const PURPLE = "text-purple-600 dark:text-purple-400";
const INDIGO = "text-indigo-600 dark:text-indigo-400";
const VIOLET = "text-violet-500";

const FALLBACK: FileIconSpec = { Icon: File, className: DIM };

/** Exact (lowercased) basenames beat extensions: manifests, git plumbing,
 * and Claude-owned files read as *that file*, not their extension. */
const BASENAMES: Record<string, FileIconSpec> = {
  "package.json": { Icon: Package, className: RED },
  "package-lock.json": { Icon: Package, className: RED },
  "pnpm-lock.yaml": { Icon: Package, className: RED },
  "yarn.lock": { Icon: Package, className: RED },
  "cargo.toml": { Icon: Package, className: ORANGE },
  "cargo.lock": { Icon: Package, className: ORANGE },
  dockerfile: { Icon: FileCog, className: SKY },
  ".gitignore": { Icon: GitBranch, className: ORANGE },
  ".gitattributes": { Icon: GitBranch, className: ORANGE },
  ".gitmodules": { Icon: GitBranch, className: ORANGE },
  license: { Icon: ScrollText, className: QUIET },
  "license.md": { Icon: ScrollText, className: QUIET },
  "license.txt": { Icon: ScrollText, className: QUIET },
  "claude.md": { Icon: BookOpenText, className: VIOLET },
};

const EXTENSIONS: Record<string, FileIconSpec> = {
  rs: { Icon: FileCode, className: ORANGE },
  ts: { Icon: FileCode, className: SKY },
  tsx: { Icon: FileCode, className: SKY },
  mts: { Icon: FileCode, className: SKY },
  cts: { Icon: FileCode, className: SKY },
  js: { Icon: FileCode, className: YELLOW },
  jsx: { Icon: FileCode, className: YELLOW },
  mjs: { Icon: FileCode, className: YELLOW },
  cjs: { Icon: FileCode, className: YELLOW },
  json: { Icon: FileJson, className: YELLOW },
  jsonc: { Icon: FileJson, className: YELLOW },
  md: { Icon: FileText, className: SKY },
  mdx: { Icon: FileText, className: SKY },
  markdown: { Icon: FileText, className: SKY },
  html: { Icon: FileCode, className: ORANGE },
  htm: { Icon: FileCode, className: ORANGE },
  css: { Icon: Palette, className: PINK },
  py: { Icon: FileCode, className: TEAL },
  go: { Icon: FileCode, className: CYAN },
  rb: { Icon: FileCode, className: RED },
  vue: { Icon: FileCode, className: EMERALD },
  svelte: { Icon: FileCode, className: ORANGE },
  java: { Icon: FileCode, className: INDIGO },
  kt: { Icon: FileCode, className: INDIGO },
  swift: { Icon: FileCode, className: INDIGO },
  c: { Icon: FileCode, className: INDIGO },
  cc: { Icon: FileCode, className: INDIGO },
  cpp: { Icon: FileCode, className: INDIGO },
  h: { Icon: FileCode, className: INDIGO },
  hpp: { Icon: FileCode, className: INDIGO },
  cs: { Icon: FileCode, className: INDIGO },
  zig: { Icon: FileCode, className: ORANGE },
  sh: { Icon: FileTerminal, className: EMERALD },
  bash: { Icon: FileTerminal, className: EMERALD },
  zsh: { Icon: FileTerminal, className: EMERALD },
  fish: { Icon: FileTerminal, className: EMERALD },
  nu: { Icon: FileTerminal, className: EMERALD },
  sql: { Icon: Database, className: CYAN },
  sqlite: { Icon: Database, className: CYAN },
  db: { Icon: Database, className: CYAN },
  toml: { Icon: FileCog, className: QUIET },
  yaml: { Icon: FileCog, className: QUIET },
  yml: { Icon: FileCog, className: QUIET },
  ini: { Icon: FileCog, className: QUIET },
  cfg: { Icon: FileCog, className: QUIET },
  conf: { Icon: FileCog, className: QUIET },
  env: { Icon: FileKey, className: YELLOW },
  pem: { Icon: FileKey, className: YELLOW },
  key: { Icon: FileKey, className: YELLOW },
  crt: { Icon: FileKey, className: YELLOW },
  pub: { Icon: FileKey, className: YELLOW },
  lock: { Icon: FileLock, className: QUIET },
  png: { Icon: FileImage, className: PURPLE },
  jpg: { Icon: FileImage, className: PURPLE },
  jpeg: { Icon: FileImage, className: PURPLE },
  gif: { Icon: FileImage, className: PURPLE },
  webp: { Icon: FileImage, className: PURPLE },
  avif: { Icon: FileImage, className: PURPLE },
  bmp: { Icon: FileImage, className: PURPLE },
  ico: { Icon: FileImage, className: PURPLE },
  svg: { Icon: FileImage, className: PURPLE },
  mp3: { Icon: FileAudio, className: PURPLE },
  wav: { Icon: FileAudio, className: PURPLE },
  flac: { Icon: FileAudio, className: PURPLE },
  ogg: { Icon: FileAudio, className: PURPLE },
  mp4: { Icon: FileVideo, className: PURPLE },
  mov: { Icon: FileVideo, className: PURPLE },
  mkv: { Icon: FileVideo, className: PURPLE },
  webm: { Icon: FileVideo, className: PURPLE },
  zip: { Icon: FileArchive, className: QUIET },
  tar: { Icon: FileArchive, className: QUIET },
  gz: { Icon: FileArchive, className: QUIET },
  tgz: { Icon: FileArchive, className: QUIET },
  bz2: { Icon: FileArchive, className: QUIET },
  xz: { Icon: FileArchive, className: QUIET },
  zst: { Icon: FileArchive, className: QUIET },
  "7z": { Icon: FileArchive, className: QUIET },
  pdf: { Icon: FileText, className: RED },
  csv: { Icon: FileSpreadsheet, className: EMERALD },
  tsv: { Icon: FileSpreadsheet, className: EMERALD },
  xlsx: { Icon: FileSpreadsheet, className: EMERALD },
  ttf: { Icon: FileType, className: QUIET },
  otf: { Icon: FileType, className: QUIET },
  woff: { Icon: FileType, className: QUIET },
  woff2: { Icon: FileType, className: QUIET },
  txt: { Icon: FileText, className: DIM },
  log: { Icon: FileText, className: DIM },
};

/** Test files read as tests wherever they live (`.test.` / `.spec.` /
 * `.e2e.` before a script extension). */
const TEST_FILE = /\.(test|spec|e2e)\.[cm]?[jt]sx?$/;

/** Icon + color for a file path (matches on its basename). */
export function fileIconSpec(path: string): FileIconSpec {
  const name = (path.split("/").pop() ?? path).toLowerCase();
  const exact = BASENAMES[name];
  if (exact) return exact;
  if (name.startsWith("readme")) return { Icon: BookOpenText, className: SKY };
  if (name.startsWith(".env")) return { Icon: FileKey, className: YELLOW };
  if (TEST_FILE.test(name)) return { Icon: FlaskConical, className: EMERALD };
  const dot = name.lastIndexOf(".");
  if (dot > 0) {
    const byExt = EXTENSIONS[name.slice(dot + 1)];
    if (byExt) return byExt;
  }
  return FALLBACK;
}

/** Special folder names whose color/icon carries meaning; anything else is a
 * neutral folder. `null` Icon = use the plain Folder/FolderOpen pair so the
 * expanded state still shows. */
const FOLDERS: Record<string, { Icon: LucideIcon | null; className: string }> = {
  ".git": { Icon: FolderGit2, className: ORANGE },
  ".github": { Icon: FolderGit2, className: ORANGE },
  ".claude": { Icon: null, className: VIOLET },
  src: { Icon: FolderCode, className: SKY },
  lib: { Icon: FolderCode, className: SKY },
  crates: { Icon: FolderCode, className: SKY },
  components: { Icon: FolderCode, className: SKY },
  test: { Icon: null, className: EMERALD },
  tests: { Icon: null, className: EMERALD },
  __tests__: { Icon: null, className: EMERALD },
  e2e: { Icon: null, className: EMERALD },
  fixtures: { Icon: null, className: EMERALD },
  docs: { Icon: null, className: BLUE },
  doc: { Icon: null, className: BLUE },
  node_modules: { Icon: null, className: "text-muted-foreground/40" },
  target: { Icon: null, className: "text-muted-foreground/40" },
  dist: { Icon: null, className: "text-muted-foreground/40" },
  build: { Icon: null, className: "text-muted-foreground/40" },
  coverage: { Icon: null, className: "text-muted-foreground/40" },
  vendor: { Icon: null, className: "text-muted-foreground/40" },
};

/** Icon + color for a folder row. Compacted chains ("a/b/c") key off their
 * last segment — that's the directory the row actually is. */
export function folderIconSpec(name: string, open: boolean): FileIconSpec {
  const last = (name.split("/").pop() ?? name).toLowerCase();
  const special = FOLDERS[last];
  const Plain = open ? FolderOpen : Folder;
  if (!special) return { Icon: Plain, className: QUIET };
  return { Icon: special.Icon ?? Plain, className: special.className };
}
