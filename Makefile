# Winter: build & test orchestration.
# Run `make help` for the target list.

.DEFAULT_GOAL := build

CMD ?= ls -la
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

UNAME := $(shell uname -s)
ifeq ($(OS),Windows_NT)
	PLATFORM := windows
else ifeq ($(UNAME),Darwin)
	PLATFORM := macos
else
	PLATFORM := linux
endif

.PHONY: help build release package install uninstall desktop dev test rust-test \
        py-test lint rust-lint py-lint fmt demo clean macos-icon extensions \
        extensions-release

help:
	@echo "winter targets:"
	@echo "  make build               Build all Rust crates (debug)"
	@echo "  make release             Build the winter binary in release mode"
	@echo "  make extensions          Build the extensions Winter ships, beside the debug binary"
	@echo "  make package             Package for the current OS:"
	@echo "                             linux   -> target/debian/winter-term_$(VERSION)-1_amd64.deb"
	@echo "                             windows -> target/winter-terminal-$(VERSION)-setup.exe"
	@echo "                             macos   -> target/winter-terminal-$(VERSION).dmg"
	@echo "  make install             Build, package, install system-wide, and add app-menu launcher"
	@echo "  make install-magic       Install system-wide with the magic-winter extension enabled"
	@echo "  make uninstall           Remove the installed package"
	@echo "  make desktop             Install a dev app-menu launcher pointing at the debug build"
	@echo "  make dev                 Build (debug) and run winter directly"
	@echo "  make test                Run every test (Rust + Python)"
	@echo "  make rust-test           Run Rust workspace tests"
	@echo "  make py-test             Run the Python client tests"
	@echo "  make lint                everything: rust-lint + py-lint"
	@echo "  make rust-lint           clippy (deny warnings)"
	@echo "  make py-lint             ruff + mypy on the Python client"
	@echo "  make fmt                 Format Rust"
	@echo "  make demo CMD='ls -la'   Run the integrated winter pipeline on a command"
	@echo "  make clean               Remove build artifacts"

FEATURES ?=
FEATURE_FLAGS := $(if $(FEATURES),--features $(FEATURES),)

# The extensions Winter ships, each the crate `crates/ext/winter-<id>`
# built as a WebAssembly component and laid out beside the binary as
# `extensions/<id>/<id>.wasm`, its package embedded, where Winter looks for
# them (see `crates/app/winter-term/src/extensions/wasm.rs`).
BUNDLED_EXTENSIONS := copilot
WASM_TARGET := wasm32-wasip2

# $(1): an extension's id; $(2): the profile directory it goes beside.
define bundle_extension
	cargo rustc -p winter-$(1) --features wasm --release --target $(WASM_TARGET) --crate-type cdylib
	@mkdir -p target/$(2)/extensions/$(1)
	@cp target/$(WASM_TARGET)/release/winter_$(1).wasm target/$(2)/extensions/$(1)/$(1).wasm

endef

build: extensions
	cargo build --workspace

release: extensions-release
	cargo build --release -p winter-term $(FEATURE_FLAGS)

extensions:
	$(foreach id,$(BUNDLED_EXTENSIONS),$(call bundle_extension,$(id),debug))

extensions-release:
	$(foreach id,$(BUNDLED_EXTENSIONS),$(call bundle_extension,$(id),release))

dev: extensions
	cargo run -p winter-term $(FEATURE_FLAGS)

install-magic:
	$(MAKE) install FEATURES=magic-winter

# ── Packaging ─────────────────────────────────────────────────────────────────

ifeq ($(PLATFORM),linux)

package: release
	@command -v cargo-deb >/dev/null 2>&1 || cargo install cargo-deb
	@rm -rf target/winter
	@cargo deb -p winter-term --no-build --no-strip --quiet
	@echo "Package: $$(ls target/debian/winter-term_$(VERSION)*.deb)"

install: package desktop
	@self_winter=""; \
	pid=$$$$; \
	while [ -n "$$pid" ] && [ "$$pid" != "1" ]; do \
		comm=$$(ps -p $$pid -o comm= 2>/dev/null); \
		if [ "$$comm" = "winter" ]; then self_winter=$$pid; break; fi; \
		pid=$$(ps -p $$pid -o ppid= 2>/dev/null | tr -d ' '); \
	done; \
	if [ -n "$$self_winter" ]; then \
		echo "Skipping pkill: this shell is running inside winter (pid $$self_winter)."; \
		echo "Restart Winter manually after install to pick up the new build."; \
	else \
		pkill -x winter >/dev/null 2>&1 || true; \
	fi
	@sudo dpkg -i $$(ls target/debian/winter-term_$(VERSION)*.deb | head -1)
	@if cargo uninstall winter-term >/dev/null 2>&1; then \
		echo "Removed the duplicate winter that cargo install put in ~/.cargo/bin."; \
	fi
	@sudo gtk-update-icon-cache -f -t /usr/share/icons/hicolor || true
	@sudo update-desktop-database /usr/share/applications || true

uninstall:
	sudo dpkg -r winter-term

desktop:
	@mkdir -p $(HOME)/.local/share/applications
	@mkdir -p $(HOME)/.local/share/icons/hicolor/64x64/apps
	@# The installed entry, pointed at the debug build and its icon.
	@sed -e 's|^Name=.*|Name=Winter Terminal (dev)|' \
		-e 's|^Exec=.*|Exec=$(CURDIR)/target/debug/winter|' \
		-e 's|^Icon=.*|Icon=$(CURDIR)/crates/app/winter-term/assets/icons/winter-terminal.png|' \
		packaging/linux/winter-terminal.desktop \
		> $(HOME)/.local/share/applications/winter-terminal-dev.desktop
	@cp crates/app/winter-term/assets/icons/winter-terminal-64.png \
		$(HOME)/.local/share/icons/hicolor/64x64/apps/winter-terminal.png
	@update-desktop-database $(HOME)/.local/share/applications || true
	@gtk-update-icon-cache -f -t $(HOME)/.local/share/icons/hicolor || true
	@echo "Launcher installed: $(HOME)/.local/share/applications/winter-terminal-dev.desktop"
	@echo "Run 'make build' first, then launch Winter from your app menu."

else ifeq ($(PLATFORM),windows)

DEB_EXE := target/winter-terminal-$(VERSION)-setup.exe
ISCC := $(shell where iscc 2>/dev/null || echo "C:/Program Files (x86)/Inno Setup 6/ISCC.exe")
export WINTER_VERSION := $(VERSION)

package: release
	@test -f "$(ISCC)" || (echo "ERROR: Inno Setup not found. Install from https://jrsoftware.org/isinfo.php" && exit 1)
	MSYS2_ARG_CONV_EXCL="*" "$(ISCC)" /Q packaging/windows/installer.iss
	@echo "Installer: $(DEB_EXE)"

# `make install` is commonly run from a terminal *inside* Winter itself
# (this is a terminal emulator). Both a direct invocation of the installer
# and a `cmd /c start`-detached one still got killed along with winter.exe
# in practice, so whatever Windows tears down along with winter.exe's ConPTY
# session reaches further than either dodge. The one thing confirmed
# independent of it is a process spawned by the Task Scheduler service, so
# the installer is handed off to that via a wrapper .bat placed next to it.
# `cmd /c echo %CD%` gets a native Windows path for schtasks's /tr without
# depending on cygpath being present. CloseApplications (installer.iss)
# closes winter.exe from inside the installer itself, once it is already
# running standalone under the scheduler. Winter sets WINTER=1 on every
# shell it spawns (see terminal/session/mod.rs), so when that is inherited here, the wrapper
# also relaunches Winter afterward via the App Paths registry entry
# installer.iss sets up.
install: package
	@restart_line=""; \
	if [ -n "$$WINTER" ]; then restart_line='start "" winter.exe'; fi; \
	printf '@echo off\r\n"%%~dp0%s" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /FORCECLOSEAPPLICATIONS\r\n%s\r\ndel "%%~f0"\r\n' \
		"$(notdir $(DEB_EXE))" "$$restart_line" > "$(dir $(DEB_EXE))run-installer.bat"
	@win_cwd="$$(MSYS2_ARG_CONV_EXCL="*" cmd /c echo %CD% | tr -d '\r')"; \
	if [ -z "$$win_cwd" ]; then echo "ERROR: could not resolve a Windows path for the installer." >&2; exit 1; fi; \
	win_bat="$$win_cwd/$(dir $(DEB_EXE))run-installer.bat"; \
	MSYS2_ARG_CONV_EXCL="*" schtasks /create /tn WinterTerminalInstall /sc once /sd 01/01/2099 /st 00:00 /tr "$$win_bat" /f || exit 1; \
	MSYS2_ARG_CONV_EXCL="*" schtasks /run /tn WinterTerminalInstall || exit 1; \
	MSYS2_ARG_CONV_EXCL="*" schtasks /delete /tn WinterTerminalInstall /f >/dev/null 2>&1 || true

uninstall:
	@echo "Run 'Add or Remove Programs' and remove 'Winter Terminal', or:"
	@"%PROGRAMFILES%\Winter Terminal\unins000.exe"

else ifeq ($(PLATFORM),macos)

APP_BUNDLE := target/Winter.app
DMG := target/winter-terminal-$(VERSION).dmg
ICONSET := target/winter.iconset

# A .app is just a directory with a known layout, so it is built directly
# rather than through cargo-bundle: one less dependency to install, and the
# layout stays visible here instead of behind a tool's own metadata format.
package: release macos-icon
	@sed 's/{{VERSION}}/$(VERSION)/g' packaging/macos/Info.plist \
		> $(APP_BUNDLE)/Contents/Info.plist
	@cp target/release/winter $(APP_BUNDLE)/Contents/MacOS/winter
	@cp -R target/release/extensions $(APP_BUNDLE)/Contents/Resources/
	@cp -R clients/shell-integration $(APP_BUNDLE)/Contents/Resources/
	@cp crates/app/winter-term/builtin/settings.kdl crates/base/winter-model/builtin/keybindings.kdl $(APP_BUNDLE)/Contents/Resources/
	@rm -f $(DMG)
	@hdiutil create -volname "Winter Terminal" -srcfolder $(APP_BUNDLE) \
		-ov -format UDZO -quiet $(DMG)
	@echo "Package: $(DMG)"

# Build the .icns from the 512px source. `sips` and `iconutil` both ship with
# macOS, so this needs nothing installed. 1024px is upscaled from 512 rather
# than omitted: Finder falls back to a smaller size and blurs it worse.
macos-icon:
	@rm -rf $(APP_BUNDLE) $(ICONSET)
	@mkdir -p $(APP_BUNDLE)/Contents/MacOS $(APP_BUNDLE)/Contents/Resources $(ICONSET)
	@for size in 16 32 128 256 512; do \
		sips -z $$size $$size crates/app/winter-term/assets/icons/winter-terminal.png \
			--out $(ICONSET)/icon_$${size}x$${size}.png >/dev/null 2>&1; \
		retina=$$((size * 2)); \
		sips -z $$retina $$retina crates/app/winter-term/assets/icons/winter-terminal.png \
			--out $(ICONSET)/icon_$${size}x$${size}@2x.png >/dev/null 2>&1; \
	done
	@iconutil -c icns $(ICONSET) -o $(APP_BUNDLE)/Contents/Resources/winter.icns
	@rm -rf $(ICONSET)

install: package
	@pkill -x winter >/dev/null 2>&1 || true
	@rm -rf /Applications/Winter.app
	@cp -R $(APP_BUNDLE) /Applications/Winter.app
	@echo "Installed to /Applications/Winter.app"
	@echo "For the 'winter' command, link it onto your PATH:"
	@echo "  sudo ln -sf /Applications/Winter.app/Contents/MacOS/winter /usr/local/bin/winter"

uninstall:
	rm -rf /Applications/Winter.app
	rm -f /usr/local/bin/winter

else

package: release
	@echo "Packaging is not configured for $(PLATFORM). Use 'make release' for a local binary."

install: release
	@pkill -x winter >/dev/null 2>&1 || true
	@install -Dm755 target/release/winter /usr/local/bin/winter
	@mkdir -p /usr/local/lib/winter
	@cp -R target/release/extensions /usr/local/lib/winter/
	@echo "Installed to /usr/local/bin/winter"

uninstall:
	rm -f /usr/local/bin/winter
	rm -rf /usr/local/lib/winter

endif

# ── Tests ─────────────────────────────────────────────────────────────────────

rust-test:
	cargo test --workspace

py-test:
	cd clients/client-py && uv run --with pytest python -m pytest -q

test: rust-test py-test

rust-lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy -p winter-term --all-targets --no-default-features -- -D warnings
	RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -A rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps --document-private-items

py-lint:
	cd clients/client-py && uv run --with ruff ruff check .
	cd clients/client-py && uv run --with mypy mypy src

lint: rust-lint py-lint

fmt:
	cargo fmt --all

demo:
	cargo run -q -p winter-term -- $(CMD)

clean:
	cargo clean
	rm -rf clients/client-py/.venv
