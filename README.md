<div align="center">
  <img src="assets/icons/icon-256.png" width="112" height="112" alt="shio" />
  <h1>shio</h1>

  <p><strong>download manager</strong></p>
</div>

---

## features

- works with direct links and torrents
- gpu rendered ui
- segmented downloads with global controls and speed limits
- automatic archive extraction
- custom themes

## install

download the latest build from [releases](https://github.com/offs/shio/releases).
beta builds are unsigned. release artifacts include sha256 files and GitHub provenance attestations.

## screenshots

<p align="center">
  <img src="docs/screenshots/screenshot1.png" alt="screenshot 1" width="100%" />
  <img src="docs/screenshots/screenshot2.png" alt="screenshot 2" width="100%" />
</p>

## build

```sh
cargo build --release -p shio-app
```

the release binary is written to `target/release/`.

## checks

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo deny check
```

## license

[MIT](LICENSE)
