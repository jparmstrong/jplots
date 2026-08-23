# jplots: build, test and release.
#
# The library is one Linux x64 shared object; `2:` finds it in the working directory, in
# $QHOME/l64, or wherever $JPLOTS_LIB points. Everything built lands under target/, and the
# recipes here point $JPLOTS_LIB at it, so nothing needs installing to run the tests.

Q       ?= q
QHOME   ?= $(HOME)/.kx
PREFIX  ?= $(QHOME)/l64
ARCH    := $(shell uname -m)
OS_NAME := $(shell uname -s)
OS      := $(shell echo $(OS_NAME) | tr A-Z a-z)
# cargo names a cdylib `.dylib` on macOS, but `2:` looks for a `.so` on every platform, so
# the built file and the installed file do not share an extension there. `OS_NAME` has to be
# set BEFORE this line: `:=` expands now, and an empty one silently picks the wrong branch.
BUILT   := target/release/libjplots$(if $(filter Darwin,$(OS_NAME)),.dylib,.so)
LIB     := target/release/libjplots
DIST    := target/dist
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

.PHONY: all build test test-rust test-q demo images install dist clean fmt lint

all: build

build:
	cargo build --release
	@echo "built $(BUILT) ($(VERSION))"

# The full gate.
test: test-rust test-q

test-rust:
	cargo test --release

test-q: build
	@if ! command -v $(Q) >/dev/null 2>&1; then echo "no q found at '$(Q)': set Q="; exit 1; fi
	@JPLOTS_LIB=$(PWD)/$(LIB) $(Q) tests/test.q -q </dev/null | grep -av '^\x1b' | tail -2

# Every chart type, on sample data, in your terminal. Needs a terminal that speaks the
# kitty graphics protocol, since the point of the demo is to look at it.
demo: build
	@JPLOTS_LIB=$(PWD)/$(LIB) $(Q) examples/demo.q -q

# The README's images, decoded from the escape stream the examples emit: the exact pixels a
# terminal would be sent, so they cannot drift from the renderer. Regenerate after a change
# that alters a chart; the diff then shows it.
images: build
	@if ! command -v $(Q) >/dev/null 2>&1; then echo "images need q: set Q="; exit 1; fi
	@python3 utils/gallery.py $$(command -v $(Q))

# `install -D` is GNU-only, so this is mkdir plus cp: the same result on both platforms.
install: build
	@mkdir -p $(PREFIX) $(QHOME)
	cp $(BUILT) $(PREFIX)/libjplots.so && chmod 755 $(PREFIX)/libjplots.so
	cp q/plt.q $(QHOME)/plt.q && chmod 644 $(QHOME)/plt.q
	@echo "installed $(PREFIX)/libjplots.so and $(QHOME)/plt.q"

# A release artifact: the library, the q front end, and the licence.
dist: build test-rust
	@rm -rf $(DIST)/jplots-$(VERSION) && mkdir -p $(DIST)/jplots-$(VERSION)
	cp $(BUILT) $(DIST)/jplots-$(VERSION)/libjplots.so
	cp q/plt.q LICENSE README.md $(DIST)/jplots-$(VERSION)/
	cd $(DIST) && tar czf jplots-$(VERSION)-$(OS)-$(ARCH).tar.gz jplots-$(VERSION)
	@echo "$(DIST)/jplots-$(VERSION)-$(OS)-$(ARCH).tar.gz"

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

clean:
	cargo clean
