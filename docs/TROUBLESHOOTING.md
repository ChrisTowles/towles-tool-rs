# Troubleshooting

## `sherpa-onnx-sys` build fails with `UnknownIssuer` behind Zscaler

```
thread 'main' panicked at .../sherpa-onnx-sys-1.13.4/build.rs:40:9:
Failed to download sherpa-onnx archive from https://github.com/k2-fsa/sherpa-onnx/releases/download/...:
Connection Failed: tls connection init failed: invalid peer certificate: UnknownIssuer
```

`sherpa-onnx-sys`'s own `build.rs` (a transitive dependency of `tt-dictate`'s
`asr` feature) downloads its prebuilt archive with `ureq 2.12` configured with
only the `proxy-from-env` feature — no `native-tls`. That means it validates
TLS against a bundled `rustls`/`webpki-roots` list instead of the OS trust
store, so it can't see the Zscaler root CA that's installed in macOS Keychain.
This is the same class of issue documented in the root `CLAUDE.md` under "TLS
clients must trust the machine's trust store" — except this time the offending
HTTP client lives inside a crate we don't control, so the workspace's own
`ureq` (already configured with `native-tls`) can't help.

Workaround: pre-download the archive with a TLS client that trusts the OS
store (`curl` on macOS uses SecureTransport/Keychain), then point the build
script at it via the `SHERPA_ONNX_ARCHIVE_DIR` env var it already checks
before attempting a network fetch:

```sh
mkdir -p ~/.cache/sherpa-onnx-archives
curl -LO https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.4/sherpa-onnx-v1.13.4-osx-arm64-static-lib.tar.bz2
mv sherpa-onnx-v1.13.4-osx-arm64-static-lib.tar.bz2 ~/.cache/sherpa-onnx-archives/
export SHERPA_ONNX_ARCHIVE_DIR=~/.cache/sherpa-onnx-archives
```

Adjust the archive filename/target triple (`osx-arm64`, `linux-x64`, etc.) and
version to match what the build error asks for. Add the `export` to your
shell profile (or a root `.env.local`, per the slots convention) so it applies
to every build.

TODO: automate this — e.g. a `tt-dictate` `build.rs` that pre-stages the
archive via the workspace's `native-tls`-backed `ureq` before
`sherpa-onnx-sys`'s build script runs, or a `tt doctor` check that detects and
offers to fetch it.
