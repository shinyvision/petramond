# Llamacraft build/run targets.
#
#   make run             -- build (release) & run the native desktop binary
#   make dev             -- build (debug) & run the native desktop binary
#   make build           -- build the release native binary
#   make clean           -- cargo clean
#   make gui-builder     -- build (release) & run the GUI builder tool
#   make gui-builder-dev -- build (debug) & run the GUI builder tool
#
# Override vars:
#   SEED=0x12345678 RD=12 make run

CARGO ?= cargo
SEED  ?= 0x2
RD    ?= 16

.PHONY: run run-native dev build build-native clean gui-builder gui-builder-dev

run: run-native
run-native: build-native
	LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --release --bin llamacraft_native

dev:
	LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --bin llamacraft_native

build: build-native
build-native:
	$(CARGO) build --release --bin llamacraft_native

clean:
	$(CARGO) clean

# Standalone data-driven GUI builder (separate crate in ./gui-builder).
gui-builder:
	cd gui-builder && $(CARGO) run --release

gui-builder-dev:
	cd gui-builder && $(CARGO) run
