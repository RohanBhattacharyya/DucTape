# DucTape build, install and packaging helpers.
# `cargo build` alone also works: build.rs builds the web UI with npm.

PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo
NPM ?= npm
APP_ID := io.github.rohanbhattacharyya.DucTape
BIN := target/release/ductape
DEV_ADDR := 127.0.0.1:7340

.PHONY: all release debug run dev ui test lint fmt install uninstall deb rpm clean

all: release

release:
	$(CARGO) build --release --locked

debug:
	$(CARGO) build

run:
	$(CARGO) run

ui:
	$(NPM) --prefix ui ci
	$(NPM) --prefix ui run build

# Hot-reloading UI at http://localhost:5173 against a headless backend
dev:
	$(CARGO) build
	./target/debug/ductape --headless --ui-addr $(DEV_ADDR) & \
		trap 'kill $$!' EXIT INT TERM; \
		DUCTAPE_BACKEND=$(DEV_ADDR) $(NPM) --prefix ui run dev

test:
	$(CARGO) test --workspace --locked

lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings
	$(NPM) --prefix ui run typecheck

fmt:
	$(CARGO) fmt --all

# Install a binary built by `make release`. Doesn't build, so it is safe to run as root.
install:
	@test -x $(BIN) || { echo "$(BIN) not found; run 'make release' first" >&2; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/ductape
	install -Dm644 packaging/linux/$(APP_ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 packaging/linux/$(APP_ID).metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	install -Dm644 packaging/linux/ductape.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(APP_ID).svg

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ductape \
		$(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop \
		$(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml \
		$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(APP_ID).svg

# Requires `cargo install cargo-deb`
deb: release
	$(CARGO) deb --no-build

# Requires `cargo install cargo-generate-rpm`
rpm: release
	$(CARGO) generate-rpm

clean:
	$(CARGO) clean
	rm -rf ui/dist ui/node_modules
