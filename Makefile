# Hydra — build and packaging entry points.
#
#   make build      release build of CLI + GUI + IPC host (any OS)
#   make app        macOS .app bundle (target/release/Hydra Download Manager.app)
#   make dmg        macOS disk image                      -> target/dist/*.dmg
#   make deb        Debian/Ubuntu package                 -> target/dist/*.deb
#   make rpm        Fedora/RHEL/openSUSE package          -> target/dist/*.rpm
#   make linux      both deb and rpm
#   make package    the right artifact(s) for the OS make runs on
#
# PROFILE=dist make build   selects the smaller panic=abort profile (Cargo.toml).

UNAME   := $(shell uname -s)
# The workspace product version ([workspace.package] in Cargo.toml), shared
# by the GUI, CLI and host bin crates that every package target bundles.
# The library crates version independently and don't affect artifact names.
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
PROFILE ?= release
CARGO   ?= cargo

.PHONY: all build cli gui host app dmg deb rpm linux windows package clean \
        require-macos require-linux

all: build

build:
	$(CARGO) build --profile $(PROFILE) -p hya-cli -p hya-gui -p hya-host

cli:
	$(CARGO) build --profile $(PROFILE) -p hya-cli

gui:
	$(CARGO) build --profile $(PROFILE) -p hya-gui

host:
	$(CARGO) build --profile $(PROFILE) -p hya-host

# Ad-hoc-signed bundle with the GUI, CLI and hydra-host inside Contents/MacOS.
# Add ARGS=--install to also replace /Applications/Hydra Download Manager.app.
app: require-macos
	scripts/macos-app-bundle.sh $(ARGS)

dmg: require-macos
	scripts/package-macos-dmg.sh

deb: require-linux
	scripts/package-linux.sh deb

rpm: require-linux
	scripts/package-linux.sh rpm

linux: require-linux
	scripts/package-linux.sh all

windows:
	scripts/build-windows-installer.sh x64

package:
ifeq ($(UNAME),Darwin)
	$(MAKE) dmg
else
	$(MAKE) linux
endif

require-macos:
	@[ "$(UNAME)" = Darwin ] || { echo "error: this target must run on macOS (found $(UNAME))" >&2; exit 1; }

require-linux:
	@[ "$(UNAME)" = Linux ] || { echo "error: this target must run on Linux (found $(UNAME))" >&2; exit 1; }

clean:
	$(CARGO) clean
	rm -rf target/dist
