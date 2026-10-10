# NexDesk build helper.  `make help` lists the targets.
# Wayland screen sharing needs libpipewire; it is switched on automatically when pkg-config finds it,
# otherwise the build continues without it (X11 only) and says so.

CARGO    ?= cargo
PROFILE  ?= release
HAS_PW   := $(shell pkg-config --exists libpipewire-0.3 2>/dev/null && echo yes)
ifeq ($(HAS_PW),yes)
  FEATURES :=
else
  FEATURES := --no-default-features
endif
# Per-target extra flags (Windows/macOS builds never use the Wayland feature)
PEER_PKGS := -p nexdesk -p nexdesk-rdp -p nexdesk-peer -p nexdesk-network
APT_DEPS  := build-essential pkg-config clang libclang-dev libpipewire-0.3-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev libxi-dev \
  libfontconfig1-dev libvulkan-dev libssl-dev

.PHONY: help deps check-deps build debug test check portable linux linux-arm64 windows macos all deb install clean

help:
	@echo "make deps        install Ubuntu/Debian build packages (sudo)"
	@echo "make build       release build of everything for this computer (default)"
	@echo "make debug       debug build"
	@echo "make test        run tests"
	@echo "make portable   viewer, agent and relay only (Windows, macOS and Linux; no GUI toolkit needed)"
	@echo "make deb         build the .deb package into dist/"
	@echo "make all         build for every platform below that is set up"
	@echo "make linux-arm64 | windows | macos   cross builds (need cargo-zigbuild, see 'make cross-help')"
	@echo "make clean"

deps:
	sudo apt-get update && sudo apt-get install -y $(APT_DEPS)

check-deps:
ifneq ($(HAS_PW),yes)
	@echo "note: libpipewire-0.3 not found - building WITHOUT Wayland sharing (X11 only)."
	@echo "      run 'make deps' to get it, then 'make build' again."
endif

build: check-deps
	$(CARGO) build --release --workspace $(FEATURES)

debug: check-deps
	$(CARGO) build --workspace $(FEATURES)

test: check-deps
	$(CARGO) test --workspace $(FEATURES)

check: check-deps
	$(CARGO) check --workspace --all-targets $(FEATURES)

linux: build

# The programs that build on every desktop OS: viewer, agent, relay.
portable:
	$(CARGO) build --release -p nexdesk-peer -p nexdesk-network --no-default-features

deb: build
	packaging/make-deb.sh

install: build
	packaging/install-local.sh

# ---- cross builds -------------------------------------------------------------------------
# Needs: rustup, zig, and `cargo install cargo-zigbuild`. Only the programs that do not depend on
# X11/Wayland are cross built: the manager, the RDP viewer and the relay. The native-peer agent and viewer
# (nexdesk-peer) capture the screen through X11/PipeWire and are Linux only for now.
ZB := cargo zigbuild --release -p nexdesk -p nexdesk-rdp -p nexdesk-network

linux-arm64:
	rustup target add aarch64-unknown-linux-gnu
	$(ZB) --target aarch64-unknown-linux-gnu

windows:
	rustup target add x86_64-pc-windows-gnu
	$(ZB) --target x86_64-pc-windows-gnu

macos:
	rustup target add aarch64-apple-darwin x86_64-apple-darwin
	$(ZB) --target universal2-apple-darwin

cross-help:
	@echo "rustup + zig (https://ziglang.org/download) + 'cargo install cargo-zigbuild'."
	@echo "macOS also needs the macOS SDK (SDKROOT=...), or run 'make build' on a Mac."
	@echo "These cross builds are untested; GPUI (the manager) may need a native Windows/macOS host."

all: build
	-$(MAKE) linux-arm64
	-$(MAKE) windows
	-$(MAKE) macos

clean:
	$(CARGO) clean
	rm -rf dist
