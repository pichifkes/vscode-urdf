# Change Log

All notable changes to the "urdf" extension will be documented in this file.

Check [Keep a Changelog](http://keepachangelog.com/) for recommendations on how to structure this file.

## [0.5.0] - 2026-05-22

First marketplace release.

- Published to VS Code Marketplace and Open VSX under publisher `Roy-Pichifkes`.
- Platform-specific .vsix per target: `linux-x64`, `darwin-x64`, `darwin-arm64`.
- Native server binary bundled inside each .vsix; no Rust toolchain required for end users.

## [Unreleased]

### 0.4.0-dev

- Reworked from a snippets-only extension into a full language extension.
- Added Rust-based language server (`urdf-lsp`, built with `tower-lsp`) and a thin TypeScript client.
- Registered `urdf` as a first-class language id (no longer aliased to `xml`); kept `.urdf` and `.xacro` file associations.
- Linux x86_64 only for now.

## [0.0.1]

- Initial release