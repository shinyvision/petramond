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
#   NV_OFFLOAD= make run        -- run on the Intel iGPU instead of the NVIDIA dGPU

CARGO ?= cargo
SEED  ?= 0x312
RD    ?= 32

# Run on the discrete NVIDIA GPU via PRIME render offload. The game renders through
# Vulkan, so __VK_LAYER_NV_optimus=NVIDIA_only (which hides the Intel iGPU from the
# Vulkan loader) is what actually steers adapter selection — the __GLX_ var only
# affects OpenGL/GLES. Override with `make run NV_OFFLOAD=` to use the Intel iGPU.
NV_OFFLOAD ?= __NV_PRIME_RENDER_OFFLOAD=1 __VK_LAYER_NV_optimus=NVIDIA_only __GLX_VENDOR_LIBRARY_NAME=nvidia

.PHONY: run run-native dev build build-native clean gui-builder gui-builder-dev

run: run-native
run-native: build-native
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
