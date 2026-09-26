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
#   make gui-builder     -- build (playtest) & run the GUI builder tool
#   make gui-builder-dev -- build (debug) & run the GUI builder tool
#   make mods            -- build mods-src (wasm32) & install packs into mods/
#   make mod ID=<name>   -- build & install one mod from mods-src/
#   make profile         -- run repeatable join + map perf harnesses in scratch data
#   make smoke           -- exercise threaded, TCP, UI-connect, and headless lifecycles
#   make test            -- the full debug-safe suite (TEST_GROUPS="core client" for a subset)
#   make test-worldgen   -- every worldgen test, the slow ignored sweeps included
#   make check           -- fmt-check, clippy, source-audit, test, genparity: what CI gates on
#   make deny            -- advisory/license/dependency-graph gate (needs cargo-deny)
#   make genparity       -- assert worldgen output matches its checked-in hash
#
# Override vars:
#   SEED=0x12345678 RD=12 make run
#   MOD_PROFILE=release make mods   -- fat-LTO guests (release packaging)
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
# Cargo profile for the wasm guests `make mods` / `make mod` build.
MOD_PROFILE ?= wasm-dev

.PHONY: run run-native run-release run-server dev build build-native clean sweep gui-builder gui-builder-dev mods mod test test-worldgen fmt fmt-check clippy deny source-audit validate-assets genparity profile smoke check

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

# Data-driven GUI builder (./gui-builder, a non-default workspace member).
# `playtest` is release-speed with incremental rebuilds.
gui-builder:
	$(CARGO) run --profile playtest -p gui-builder

gui-builder-dev:
	$(CARGO) run -p gui-builder

# Build every mod crate in mods-src/ (its own wasm32 workspace) and install
# each one that ships a pack/ dir into mods/<id>/ (pack files + mod.wasm),
# where the game discovers it; only files that changed are copied. The pack
# convention and its checks live in scripts/install-mods.sh, shared with the
# tests and releases. MOD_PROFILE=wasm-dev (default) is the fast iteration
# build; release packaging uses MOD_PROFILE=release.
# `make mod ID=<name>` builds and installs a single mod.
mods:
	CARGO_CMD="$(CARGO)" MOD_PROFILE="$(MOD_PROFILE)" bash scripts/install-mods.sh mods

mod:
	@[ -n "$(ID)" ] || { echo "usage: make mod ID=<mod-id>" >&2; exit 2; }
	CARGO_CMD="$(CARGO)" MOD_PROFILE="$(MOD_PROFILE)" bash scripts/install-mods.sh mods $(ID)

# The canonical suite builds bundled WASM guests into target/, installs their
# packs into an isolated temporary root, and runs every workspace with debug
# assertions and overflow checks enabled. It never reads a developer's mods/.
test:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/test-all.sh $(TEST_GROUPS)

# Every worldgen test, the slow `#[ignore]`d genmap sweeps included. A plain
# `cargo test -p petramond-worldgen` runs everything but those sweeps.
test-worldgen:
	CARGO_CMD="$(CARGO)" bash scripts/test-all.sh worldgen

# Two workspaces: the root one (game, GUI builder, guest SDK) and mods-src.
fmt:
	$(CARGO) fmt --all
	$(CARGO) fmt --manifest-path mods-src/Cargo.toml --all

fmt-check:
	$(CARGO) fmt --all -- --check
	$(CARGO) fmt --manifest-path mods-src/Cargo.toml --all -- --check

# The lint policy itself lives in [workspace.lints] (so editors agree with
# CI); `-D warnings` makes every warning fatal here.
clippy:
	$(CARGO) clippy --workspace --all-targets --features petramond-client/tools,petramond-worldgen/tools -- -D warnings
	$(CARGO) clippy --manifest-path mods-src/Cargo.toml --target-dir target --workspace --all-targets -- -D warnings

# Supply-chain gate over both lockfiles: RUSTSEC advisories, licenses,
# duplicate versions, crate sources and the crate-graph bans in deny.toml.
# Needs `cargo install --locked cargo-deny`; CI runs it on every change and
# weekly.
deny:
	$(CARGO) deny --config "$(CURDIR)/deny.toml" --workspace check
	$(CARGO) deny --config "$(CURDIR)/deny.toml" --manifest-path mods-src/Cargo.toml --workspace check

# Keep giant test fixtures and declarative wire schemas from disguising the
# size of executable modules, and stop production modules growing past the
# reviewable ceiling without first extracting a cohesive submodule.
source-audit:
	bash scripts/audit-source.sh

# A quick gate over shipped data and shaders while editing assets. Every test
# it names also runs in `test`, so `check` (and CI) leave it out.
validate-assets:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/validate-assets.sh

# Worldgen byte-parity gate: the production section pipeline's hash over a
# fixed sample must equal EXPECTED_COMBINED in
# crates/petramond-worldgen/src/parity.rs. Release-speed codegen, and an empty
# mods root so no installed pack can shape the terrain it hashes. A change
# meant to alter generation updates that constant in the same commit.
GENPARITY_MODS := $(CURDIR)/target/genparity-mods
genparity:
	mkdir -p "$(GENPARITY_MODS)"
	PETRAMOND_MODS="$(GENPARITY_MODS)" \
		$(CARGO) run --quiet --profile playtest -p petramond-worldgen --bin genparity

# Manual measurement targets are intentionally outside `check`: profile
# numbers are machine/load dependent, and smoke duplicates full-suite coverage.
profile:
	MODS_PROFILE=release CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/profile.sh

smoke:
	CARGO_CMD="$(CARGO)" bash scripts/with-test-mods.sh bash scripts/smoke.sh

check: fmt-check clippy source-audit test genparity
