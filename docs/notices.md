# Updating dependency notices

`THIRD_PARTY_NOTICES.html` is generated with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.2, using
`about.toml`, `about.hbs` and the committed Cargo.lock. Runtime and build
dependencies for Windows x64 are included; development-only dependencies are
excluded. Do not reuse old notices after changing dependencies without review.

```powershell
cargo about generate --locked --fail --output-file THIRD_PARTY_NOTICES.html about.hbs
```

The tool used for the first public release was the upstream Windows x64 archive,
SHA-256 `1c03e5890238562497c2d89a3b75b02560af349c1fc3e713d3284f532a5cd748`.
The archive is tooling, not part of the Switchbite source or binary package.

`RUST_STANDARD_LIBRARY_NOTICES.html` is copied verbatim from the Rust 1.99.1 MSVC
toolchain's `share/doc/rust/COPYRIGHT-library.html`. Refresh it when changing the
release toolchain. Both notice files and the project MIT license are included in
the binary ZIP and hashed by its manifest.
