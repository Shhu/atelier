# Atelier

A light image editor with familiar shortcuts: non-destructive layers,
retouching, type, vectors, color management and local AI selection, in
English and French. Written in Rust with [egui](https://github.com/emilk/egui).

Downloads and the website: <https://shhu.github.io/atelier/>

## Building

Atelier needs a recent stable Rust toolchain (edition 2024).

```sh
cargo run --release
cargo test
```

On Linux, the AI features download ONNX Runtime at build time (the `ort`
crate); on Windows it is embedded in the executable (see
`packaging/windows/`). The AI models themselves are downloaded by the app,
only after the user accepts, from the AI Models window.

Packaging scripts:

- Windows, from WSL: `packaging/windows/build.sh` (a single `atelier.exe`).
- Linux: `packaging/flatpak/build.sh` (a Flatpak bundle).
- macOS (Apple silicon): built by the `macos.yml` GitHub workflow.

## Contributing

- Code, comments and commit messages in English.
- Every text shown to users goes through `t!` (`src/i18n.rs`), with its key in
  both `locales/en.ftl` and `locales/fr.ftl` (same keys, same order); the
  tests check that every key is translated and used.
- Behaviors, shortcuts and names follow the usual conventions of professional
  image editors.
- `cargo fmt`, `cargo clippy --all-targets` and `cargo test` must pass. The
  app-level tests drive the interface with `egui_kittest`; the ignored
  `capture` test renders screenshots (options at the end of `src/app/mod.rs`).

## License

Atelier is licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in Atelier
shall be dual licensed as above, without any additional terms or conditions.

Third-party works embedded in the binaries are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
