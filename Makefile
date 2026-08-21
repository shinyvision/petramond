# Petramond build/run targets.
#
#   make run             -- build (playtest: release-speed, fast rebuilds) & run
#   make run-release     -- build (full release: thin LTO, 1 CGU) & run
#   make run-server      -- build (playtest) & run the headless dedicated server
#                           (WORLD=<name> required; PORT=7434 SEED= RD= optional)
#   make dev             -- build (debug) & run the native desktop binary
#   make build           -- build the release native binary
#   make clean           -- cargo clean
#   make gui-builder     -- build (release) & run the GUI builder tool
#   make gui-builder-dev -- build (debug) & run the GUI builder tool
#   make mods            -- build mods-src (wasm32) & install packs into mods/
#   make profile         -- run repeatable join + map perf harnesses in scratch data
#   make smoke           -- exercise threaded, TCP, UI-connect, and headless lifecycles
#
# Override vars:
#   SEED=0x12345678 RD=12 make run
#   NV_OFFLOAD= make run        -- run on the Intel iGPU instead of the NVIDIA dGPU
#
# RD is only exported when set explicitly: the client normally reads the view
# distance saved in client.json (forcing PETRAMOND_RD on every run shadowed
# the Options slider across restarts). The headless server has no client.json,
# so it falls back to 32.

CARGO ?= cargo
SEED  ?= 0x312
RD    ?=

# Run on the discrete NVIDIA GPU via PRIME render offload. The game renders through
# Vulkan, so __VK_LAYER_NV_optimus=NVIDIA_only (which hides the Intel iGPU from the
# Vulkan loader) is what actually steers adapter selection — the __GLX_ var only
# affects OpenGL/GLES. Override with `make run NV_OFFLOAD=` to use the Intel iGPU.
NV_OFFLOAD ?= __NV_PRIME_RENDER_OFFLOAD=1 __VK_LAYER_NV_optimus=NVIDIA_only __GLX_VENDOR_LIBRARY_NAME=nvidia

.PHONY: run run-native run-release run-server dev build build-native clean gui-builder gui-builder-dev mods test fmt fmt-check clippy source-audit validate-assets profile smoke check

# `run` uses the `playtest` profile: release opt-level but incremental with
# parallel codegen units and no LTO, so the edit→playtest loop rebuilds in
# seconds. `run-release` is the exact shipped configuration.
run: run-native
run-native:
	$(NV_OFFLOAD) PETRAMOND_SEED=$(SEED) $(if $(RD),PETRAMOND_RD=$(RD)) \
		$(CARGO) run --profile playtest -p petramond-client --bin petramond_native

run-release: build-native
	$(NV_OFFLOAD) PETRAMOND_SEED=$(SEED) $(if $(RD),PETRAMOND_RD=$(RD)) \
		$(CARGO) run --release -p petramond-client --bin petramond_native

# Headless dedicated server (no GPU, no window, no audio libs — the engine
# crate's tree simply has none, so there is no feature to switch off).
# `make run-server WORLD=myworld`.
PORT ?= 7434
run-server:
	@test -n "$(WORLD)" || { echo "usage: make run-server WORLD=<world-name> [PORT=7434]"; exit 2; }
	PETRAMOND_SEED=$(SEED) PETRAMOND_RD=$(or $(RD),32) PETRAMOND_PORT=$(PORT) \
		$(CARGO) run --profile playtest -p petramond --bin petramond_server -- $(WORLD)

dev:
	$(NV_OFFLOAD) PETRAMOND_SEED=$(SEED) $(if $(RD),PETRAMOND_RD=$(RD)) \
		$(CARGO) run -p petramond-client --bin petramond_native

build: build-native
build-native:
	$(CARGO) build --release -p petramond-client --bin petramond_native

clean:
	$(CARGO) clean

# Standalone data-driven GUI builder (separate crate in ./gui-builder).
gui-builder:
	$(CARGO) run --manifest-path gui-builder/Cargo.toml --target-dir target --release

gui-builder-dev:
	$(CARGO) run --manifest-path gui-builder/Cargo.toml --target-dir target

# Build every mod crate in mods-src/ (its own wasm32 workspace) and install
# each one that ships a pack/ dir into mods/<id>/ (pack files + mod.wasm),
# where the game discovers it. Convention: crate name == directory name == the
# mod id in pack/pack.json. Crates without a pack/ dir (test fixtures) are
# built but not installed. A pack with no compiled wasm installs as
# content-only, which the mod API supports.
mods:
	$(CARGO) build --manifest-path mods-src/Cargo.toml --target-dir target \
		--release --target wasm32-unknown-unknown
	@set -e; for d in mods-src/*/; do \
		id=$$(basename $$d); \
		wasm_id=$$(printf '%s' "$$id" | tr '-' '_'); \
		[ -f "$$d/pack/pack.json" ] || continue; \
		mkdir -p mods/$$id; \
		cp -r $$d/pack/. mods/$$id/; \
		if [ -f target/wasm32-unknown-unknown/release/$$wasm_id.wasm ]; then \
			cp target/wasm32-unknown-unknown/release/$$wasm_id.wasm mods/$$id/mod.wasm; \
			echo "installed mods/$$id"; \
		else \
			echo "installed mods/$$id (content only, no wasm)"; \
		fi; \
	done

# The canonical suite builds bundled WASM guests into target/, installs their
# packs into an isolated temporary root, and runs every workspace with debug
# assertions and overflow checks enabled. It never reads a developer's mods/.
test:
	CARGO="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/test-all.sh

fmt:
	$(CARGO) fmt --all
	$(CARGO) fmt --manifest-path mods-src/Cargo.toml --all
	$(CARGO) fmt --manifest-path mod-sdk/Cargo.toml
	$(CARGO) fmt --manifest-path gui-builder/Cargo.toml

fmt-check:
	$(CARGO) fmt --all -- --check
	$(CARGO) fmt --manifest-path mods-src/Cargo.toml --all -- --check
	$(CARGO) fmt --manifest-path mod-sdk/Cargo.toml -- --check
	$(CARGO) fmt --manifest-path gui-builder/Cargo.toml -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) clippy --manifest-path mods-src/Cargo.toml --target-dir target --workspace --all-targets -- -D warnings
	$(CARGO) clippy --manifest-path mod-sdk/Cargo.toml --target-dir target --all-targets -- -D warnings
	$(CARGO) clippy --manifest-path gui-builder/Cargo.toml --target-dir target --all-targets -- -D warnings

# Keep giant test fixtures and declarative wire schemas from disguising the
# size of executable modules, and stop production modules growing past the
# reviewable ceiling without first extracting a cohesive submodule.
source-audit:
	bash scripts/audit-source.sh

validate-assets:
	CARGO="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/validate-assets.sh

# Manual measurement targets are intentionally outside `check`: profile
# numbers are machine/load dependent, and smoke duplicates full-suite coverage.
profile:
	CARGO="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/profile.sh

smoke:
	CARGO="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/smoke.sh

check: fmt-check clippy source-audit test validate-assets
