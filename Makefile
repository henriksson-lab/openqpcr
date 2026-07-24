# openqpcr — build & packaging Makefile
#
# Modeled on the sibling `traceanalyzer` project's packaging Makefile. openqpcr
# is a Cargo workspace that ships two binaries:
#   * openqpcr-gui  (package openqpcr-gui) — the Slint desktop app (primary)
#   * openqpcr      (package openqpcr)      — the CLI reader/viewer
# The GUI is the primary shippable app; the CLI is packaged alongside it.
#
# Packaging parity with traceanalyzer (pure make + shell + CI, no cargo-* crates):
#   * Linux  -> `make install` / `make uninstall` (freedesktop layout)
#             + `make deb` (Debian package via dpkg-deb; also built in CI)
#   * macOS  -> `make osx-app`   (or scripts/build-macos-app.sh; macOS only)
#   * Windows-> built & zipped in CI (.github/workflows/build-artifacts.yml)

APP_NAME := openqpcr
GUI_BINARY := openqpcr-gui
CLI_BINARY := openqpcr
BUNDLE_ID := org.openqpcr.OpenQPCR
VERSION := 0.1.0

# --- macOS .app bundle layout -----------------------------------------------
# The GUI is the bundle's main executable; the CLI is dropped in alongside it.
APP_BUNDLE := target/osx/$(APP_NAME).app
GUI_RELEASE := target/release/$(GUI_BINARY)
CLI_RELEASE := target/release/$(CLI_BINARY)
APP_EXE := $(APP_BUNDLE)/Contents/MacOS/$(GUI_BINARY)
APP_CLI_EXE := $(APP_BUNDLE)/Contents/MacOS/$(CLI_BINARY)
APP_PLIST := $(APP_BUNDLE)/Contents/Info.plist
# App icon: assets/icon.svg is the source of truth; assets/icon-1024.png is the
# committed 1024px master it renders to. The .icns is built from that master at
# packaging time with macOS built-ins only (sips + iconutil) — no SVG rasterizer.
# NOTE: assets/icon.svg + assets/icon-1024.png are placeholder icons; drop a real
# design in their place (keep the same paths) when one is available.
APP_ICON_SRC := assets/icon-1024.png
APP_ICONSET := target/osx/AppIcon.iconset
APP_ICNS := $(APP_BUNDLE)/Contents/Resources/AppIcon.icns

# --- Linux install (freedesktop layout) -------------------------------------
# Standard prefix vars; packagers override DESTDIR (staging) and PREFIX.
PREFIX ?= /usr/local
DESTDIR ?=
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
ICONDIR := $(DATADIR)/icons/hicolor
# The window's Wayland app_id / X11 WM_CLASS; Slint uses the binary name by
# default, so the icon PNG/SVG and the .desktop StartupWMClass are all keyed to
# the GUI binary name so the desktop finds the icon.
LINUX_APP_ID := openqpcr-gui

# Extra args forwarded to `make run-cli` (e.g. `make run-cli ARGS="summary run.xlsx"`).
ARGS ?=

.PHONY: build release test run run-cli clean \
	install uninstall deb osx-app clean-osx-app

# --- Developer convenience targets ------------------------------------------
build:
	cargo build --workspace

release:
	cargo build --workspace --release

test:
	cargo test --workspace

# Run the GUI (primary app).
run:
	cargo run -p $(GUI_BINARY)

# Run the CLI, e.g. `make run-cli ARGS="summary path/to/export_dir/"`.
run-cli:
	cargo run -p $(CLI_BINARY) -- $(ARGS)

clean:
	cargo clean
	rm -rf target/osx dist

# --- Linux install ----------------------------------------------------------
# Install both release binaries, the .desktop entry, and the icons into a
# freedesktop layout. Uses the scalable SVG (any size, no rasterizer needed)
# plus the committed 256px PNG as a fallback for themes that ignore SVG.
install:
	cargo build -p $(GUI_BINARY) --release
	cargo build -p $(CLI_BINARY) --release
	install -Dm755 "$(GUI_RELEASE)" "$(BINDIR)/$(GUI_BINARY)"
	install -Dm755 "$(CLI_RELEASE)" "$(BINDIR)/$(CLI_BINARY)"
	install -Dm644 packaging/openqpcr.desktop \
		"$(DATADIR)/applications/$(LINUX_APP_ID).desktop"
	install -Dm644 assets/icon.svg \
		"$(ICONDIR)/scalable/apps/$(LINUX_APP_ID).svg"
	install -Dm644 gui/assets/window-icon.png \
		"$(ICONDIR)/256x256/apps/$(LINUX_APP_ID).png"
	@printf 'Installed to %s. If not staging (DESTDIR empty), refresh caches:\n' "$(DESTDIR)$(PREFIX)"
	@printf '  update-desktop-database %s/applications\n' "$(DATADIR)"
	@printf '  gtk-update-icon-cache %s\n' "$(ICONDIR)"

uninstall:
	rm -f "$(BINDIR)/$(GUI_BINARY)" \
		"$(BINDIR)/$(CLI_BINARY)" \
		"$(DATADIR)/applications/$(LINUX_APP_ID).desktop" \
		"$(ICONDIR)/scalable/apps/$(LINUX_APP_ID).svg" \
		"$(ICONDIR)/256x256/apps/$(LINUX_APP_ID).png"

# --- Linux .deb package -----------------------------------------------------
# Stage the freedesktop layout via `make install` into a package root, then wrap
# it with dpkg-deb. Runtime `Depends:` are derived from the actual ELF binaries
# by mapping their linked shared libraries to the owning packages (ldd + dpkg -S)
# — pure dpkg/coreutils tooling, no cargo-* crates (packaging parity, see header).
DEB_ARCH ?= $(shell dpkg --print-architecture 2>/dev/null || echo amd64)
DEB_STAGE := target/deb/openqpcr_$(VERSION)_$(DEB_ARCH)
DEB_FILE := dist/openqpcr_$(VERSION)_$(DEB_ARCH).deb
DEB_MAINTAINER ?= Johan Henriksson <johan.henriksson@umu.se>
DEB_HOMEPAGE ?= https://github.com/henriksson-lab/openqpcr

deb:
	rm -rf "$(DEB_STAGE)"
	mkdir -p "$(DEB_STAGE)/DEBIAN" dist
	$(MAKE) install DESTDIR="$(DEB_STAGE)" PREFIX=/usr
	@# Map each binary's linked shared libraries to their providing packages.
	deps=$$(for b in "$(DEB_STAGE)/usr/bin/$(GUI_BINARY)" "$(DEB_STAGE)/usr/bin/$(CLI_BINARY)"; do \
		ldd "$$b" 2>/dev/null | awk '/=> \//{print $$3}'; \
	  done | sort -u | xargs -r dpkg -S 2>/dev/null | cut -d: -f1 | tr ',' '\n' \
	  | sort -u | grep -v '^$$' | paste -sd, -); \
	[ -n "$$deps" ] || deps=libc6; \
	size=$$(du -ks "$(DEB_STAGE)/usr" | cut -f1); \
	{ \
	  echo "Package: openqpcr"; \
	  echo "Version: $(VERSION)"; \
	  echo "Section: science"; \
	  echo "Priority: optional"; \
	  echo "Architecture: $(DEB_ARCH)"; \
	  echo "Maintainer: $(DEB_MAINTAINER)"; \
	  echo "Installed-Size: $$size"; \
	  echo "Depends: $$deps"; \
	  echo "Homepage: $(DEB_HOMEPAGE)"; \
	  echo "Description: Reader and viewer for Bio-Rad CFX real-time PCR data"; \
	  echo " openqpcr reads Bio-Rad CFX (Connect / Duet / Opus) qPCR data — CSV/Excel"; \
	  echo " exports and RDML — into a shared model, exposed through a CLI (openqpcr)"; \
	  echo " and a Slint desktop GUI (openqpcr-gui) modeled on CFX Maestro."; \
	} > "$(DEB_STAGE)/DEBIAN/control"
	dpkg-deb --build --root-owner-group "$(DEB_STAGE)" "$(DEB_FILE)"
	@printf 'Built %s\n' "$(DEB_FILE)"

# --- macOS .app bundle ------------------------------------------------------
# macOS only: uses sips + iconutil (Apple built-ins) to build the .icns.
osx-app:
	cargo build -p $(GUI_BINARY) --release
	cargo build -p $(CLI_BINARY) --release
	mkdir -p "$(APP_BUNDLE)/Contents/MacOS" "$(APP_BUNDLE)/Contents/Resources" "$(APP_ICONSET)"
	cp "$(GUI_RELEASE)" "$(APP_EXE)"
	cp "$(CLI_RELEASE)" "$(APP_CLI_EXE)"
	chmod +x "$(APP_EXE)" "$(APP_CLI_EXE)"
	set -e; for pair in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" \
		"64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" \
		"256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" \
		"1024 icon_512x512@2x"; do \
		set -- $$pair; \
		sips -z $$1 $$1 "$(APP_ICON_SRC)" --out "$(APP_ICONSET)/$$2.png" >/dev/null; \
	done
	iconutil -c icns "$(APP_ICONSET)" -o "$(APP_ICNS)"
	printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0">' \
		'<dict>' \
		'  <key>CFBundleDevelopmentRegion</key>' \
		'  <string>en</string>' \
		'  <key>CFBundleDisplayName</key>' \
		'  <string>$(APP_NAME)</string>' \
		'  <key>CFBundleExecutable</key>' \
		'  <string>$(GUI_BINARY)</string>' \
		'  <key>CFBundleIconFile</key>' \
		'  <string>AppIcon</string>' \
		'  <key>CFBundleIdentifier</key>' \
		'  <string>$(BUNDLE_ID)</string>' \
		'  <key>CFBundleInfoDictionaryVersion</key>' \
		'  <string>6.0</string>' \
		'  <key>CFBundleName</key>' \
		'  <string>$(APP_NAME)</string>' \
		'  <key>CFBundlePackageType</key>' \
		'  <string>APPL</string>' \
		'  <key>CFBundleShortVersionString</key>' \
		'  <string>$(VERSION)</string>' \
		'  <key>CFBundleVersion</key>' \
		'  <string>$(VERSION)</string>' \
		'  <key>LSMinimumSystemVersion</key>' \
		'  <string>11.0</string>' \
		'  <key>NSHighResolutionCapable</key>' \
		'  <true/>' \
		'  <key>CFBundleDocumentTypes</key>' \
		'  <array>' \
		'    <dict>' \
		'      <key>CFBundleTypeName</key>' \
		'      <string>Bio-Rad CFX qPCR data</string>' \
		'      <key>CFBundleTypeRole</key>' \
		'      <string>Viewer</string>' \
		'      <key>LSHandlerRank</key>' \
		'      <string>Alternate</string>' \
		'      <key>CFBundleTypeExtensions</key>' \
		'      <array>' \
		'        <string>csv</string>' \
		'        <string>xlsx</string>' \
		'      </array>' \
		'    </dict>' \
		'  </array>' \
		'</dict>' \
		'</plist>' \
		> "$(APP_PLIST)"
	@printf 'Built %s\n' "$(APP_BUNDLE)"

clean-osx-app:
	rm -rf "$(APP_BUNDLE)" "$(APP_ICONSET)"
