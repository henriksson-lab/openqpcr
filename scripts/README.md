# scripts

## build-macos-app.sh

Packages the release openqpcr binaries into a macOS `.app` bundle (and,
optionally, a `.dmg`) under `dist/` at the repo root. macOS only.

```sh
bash scripts/build-macos-app.sh          # build dist/openqpcr.app
bash scripts/build-macos-app.sh --dmg    # also build dist/openqpcr-<version>.dmg
```

The script runs `cargo build --release -p openqpcr-gui -p openqpcr`, assembles
`openqpcr.app/Contents/{MacOS,Resources}` with the Slint GUI (`openqpcr-gui`) as
the bundle's main executable and the CLI (`openqpcr`) alongside it, and writes an
`Info.plist` declaring `.csv`/`.xlsx` as openable document types. It is
idempotent (the bundle is rebuilt from scratch each run) and validates the
generated plist with `plutil -lint`.

The bundle is **unsigned and un-notarized**, so Gatekeeper will warn on first
launch; see the commented `codesign --deep --sign` note in the script for future
signing. This standalone script ships iconless unless you drop a prebuilt
`scripts/openqpcr.icns`. Alternatively, `make osx-app` builds the `.icns` from
`assets/icon-1024.png` using Apple's `sips` + `iconutil`. The icon assets under
`assets/` are placeholders — replace them with a real design when available. The
`dist/` output directory is git-ignored.
