# Llamacraft build/run targets.
#
#   make run             -- build (playtest: release-speed, fast rebuilds) & run
#   make run-release     -- build (full release: thin LTO, 1 CGU) & run
#   make dev             -- build (debug) & run the native desktop binary
#   make build           -- build the release native binary
#   make clean           -- cargo clean
#   make gui-builder     -- build (release) & run the GUI builder tool
#   make gui-builder-dev -- build (debug) & run the GUI builder tool
#   make mods            -- build mods-src (wasm32) & install packs into mods/
#
# Override vars:
#   SEED=0x12345678 RD=12 make run
#   NV_OFFLOAD= make run        -- run on the Intel iGPU instead of the NVIDIA dGPU

CARGO ?= cargo
SEED  ?= 0x312
RD    ?= 32

# Run on the discrete NVIDIA GPU via PRIME render offload. The game renders through
# Vulkan, so __VK_LAYER_NV_optimus=NVIDIA_only (which hides the Intel iGPU from the
# Vulkan loader) is what actually steers adapter selection — the __GLX_ var only
# affects OpenGL/GLES. Override with `make run NV_OFFLOAD=` to use the Intel iGPU.
NV_OFFLOAD ?= __NV_PRIME_RENDER_OFFLOAD=1 __VK_LAYER_NV_optimus=NVIDIA_only __GLX_VENDOR_LIBRARY_NAME=nvidia

.PHONY: run run-native run-release dev build build-native clean gui-builder gui-builder-dev mods

# `run` uses the `playtest` profile: release opt-level but incremental with
# parallel codegen units and no LTO, so the edit→playtest loop rebuilds in
# seconds. `run-release` is the exact shipped configuration.
run: run-native
run-native:
	$(NV_OFFLOAD) LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --profile playtest --bin llamacraft_native

run-release: build-native
	$(NV_OFFLOAD) LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --release --bin llamacraft_native

dev:
	$(NV_OFFLOAD) LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
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

# Build every mod crate in mods-src/ (its own wasm32 workspace) and install
# each one that ships a pack/ dir into mods/<id>/ (pack files + mod.wasm),
# where the game discovers it. Convention: crate name == directory name == the
# mod id in pack/pack.json. Crates without a pack/ dir (test fixtures) are
# built but not installed.
mods:
	cd mods-src && $(CARGO) build --release --target wasm32-unknown-unknown
	@set -e; for d in mods-src/*/; do \
		id=$$(basename $$d); \
		[ -f "$$d/pack/pack.json" ] || continue; \
		mkdir -p mods/$$id; \
		cp -r $$d/pack/. mods/$$id/; \
		cp mods-src/target/wasm32-unknown-unknown/release/$$id.wasm mods/$$id/mod.wasm; \
		echo "installed mods/$$id"; \
	done
