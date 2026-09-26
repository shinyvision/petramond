# Petramond build/run targets.
#
#   make run             -- build (playtest: release-speed, fast rebuilds) & run
#   make run-release     -- build (full release: thin LTO, 1 CGU) & run
#   make run-server      -- build (playtest) & run the headless dedicated server
#                           (WORLD=<name> required; PORT=7434 SEED= RD= optional)
#   make dev             -- build (debug) & run the native desktop binary
#   make build           -- build the release native binary
#   make clean           -- cargo clean
#   make sweep           -- delete build artifacts unused for SWEEP_DAYS (default 3) days
#   make gui-builder     -- build (release) & run the GUI builder tool
#   make gui-builder-dev -- build (debug) & run the GUI builder tool
#   make mods            -- build mods-src (wasm32) & install packs into mods/
#   make profile         -- run repeatable join + map perf harnesses in scratch data
#   make smoke           -- exercise threaded, TCP, UI-connect, and headless lifecycles
#   make test            -- the full debug-safe suite (TEST_GROUPS="core client" for a subset)
#   make check           -- fmt-check, clippy, source-audit, test: what CI gates on
#
# Override vars:
#   SEED=0x12345678 RD=12 make run
#   TEST_GROUPS=worldgen make test  -- groups are listed in scripts/test-all.sh
#
# RD is only exported when set explicitly: the client normally reads the view
# distance saved in client.json (forcing PETRAMOND_RD on every run shadowed
# the Options slider across restarts). The headless server has no client.json,
# so it falls back to 32.

# Machine-specific tuning is opt-in and never committed. Everything here and in
# .cargo/config.toml stays portable, because CI and release builds use the same
# files. Put per-machine settings in an untracked `local.mk` next to this file
# (see local.mk.example: niced builds, a GPU-offload prefix for launches). Put
# cargo settings that should hold for EVERY cargo invocation, make or no make
# (a `jobs` cap, `target-cpu`), in your user-level ~/.cargo/config.toml.
-include local.mk

CARGO ?= cargo
# Cargo reads $CARGO as the path to its own executable (and hands it to build
# scripts and tests), so a multi-word command must never reach it as that
# variable. The scripts take the command as CARGO_CMD instead.
unexport CARGO
SEED  ?= 0x312
RD    ?=
# Env-var prefix for the commands that launch the game window (run, dev,
# run-release), e.g. a PRIME render-offload selection. Empty by default.
NV_OFFLOAD ?=
TEST_GROUPS ?=

.PHONY: run run-native run-release run-server dev build build-native clean sweep gui-builder gui-builder-dev mods test fmt fmt-check clippy source-audit validate-assets profile smoke check

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

# Delete build artifacts no build has used for SWEEP_DAYS days, across every
# target dir in the repo. Needs `cargo install cargo-sweep`.
SWEEP_DAYS ?= 3
sweep:
	CARGO_CMD="$(CARGO)" SWEEP_DAYS=$(SWEEP_DAYS) bash scripts/sweep.sh

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
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/test-all.sh $(TEST_GROUPS)

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

# A quick gate over shipped data and shaders while editing assets. Every test
# it names also runs in `test`, so `check` (and CI) leave it out.
validate-assets:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/validate-assets.sh

# Manual measurement targets are intentionally outside `check`: profile
# numbers are machine/load dependent, and smoke duplicates full-suite coverage.
profile:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/profile.sh

smoke:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/smoke.sh

check: fmt-check clippy source-audit test
