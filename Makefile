# Llamacraft build/run targets.
#
#   make run     -- build & run native desktop binary
#   make web     -- build wasm + web bundle, serve + open browser
#   make build   -- build both native + web bundle
#   make clean   -- cargo clean + wipe generated web artifacts
#
# Override vars:
#   SEED=0x12345678 RD=12 make run
#   PORT=9000 BROWSER=firefox make web

CARGO      ?= cargo
PORT       ?= 8070
BROWSER    ?= xdg-open
SEED       ?= 0x12345678
RD         ?= 16
WEB_DIR    := web
SRV_LOG    := /tmp/llamacraft_dev_server.log
WEB_READY  := http://localhost:$(PORT)/

.PHONY: run run-native web web-build web-serve web-open build build-native build-web clean stop

# ----------------------------------------------------------------------------
# Native desktop
# ----------------------------------------------------------------------------
run: run-native

dev:
	LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --bin llamacraft_native

run-native: build-native
	LLAMACRAFT_SEED=$(SEED) LLAMACRAFT_RD=$(RD) \
		$(CARGO) run --release --bin llamacraft_native

build-native:
	$(CARGO) build --release --bin llamacraft_native

# ----------------------------------------------------------------------------
# Web (wasm + dev server + browser)
# ----------------------------------------------------------------------------
web: web-build stop web-serve web-open

web-build:
	@scripts/build_web.sh

web-serve:
	@echo "==> dev server on $(WEB_READY) (log: $(SRV_LOG))"
	@cd $(WEB_DIR) && PORT=$(PORT) nohup python3 dev_server.py \
		>$(SRV_LOG) 2>&1 & disown 2>/dev/null || true
	@until curl -sf $(WEB_READY) >/dev/null 2>&1; do \
		sleep 0.2; \
	done
	@echo "==> server up"

web-open:
	@$(BROWSER) $(WEB_READY) || \
		echo "(could not open browser; visit $(WEB_READY) manually)"

# Stop any running dev server (best-effort, ignore errors).
stop:
	@-pkill -f "dev_server.py" 2>/dev/null || true

# ----------------------------------------------------------------------------
# Aggregate
# ----------------------------------------------------------------------------
build: build-native build-web

build-web: web-build

clean: stop
	$(CARGO) clean
	rm -f $(WEB_DIR)/llamacraft.js $(WEB_DIR)/llamacraft.d.ts \
	      $(WEB_DIR)/llamacraft_bg.wasm $(WEB_DIR)/llamacraft_bg.wasm.d.ts \
	      $(WEB_DIR)/worker_wasm.js $(WEB_DIR)/worker_wasm.d.ts \
	      $(WEB_DIR)/worker_wasm_bg.wasm $(WEB_DIR)/worker_wasm_bg.wasm.d.ts \
	      $(WEB_DIR)/atlas.png
