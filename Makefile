OUTPUT_DIR := Output
BINARY := Formicary

.PHONY: all build setup-env build-ui build-server run clean clean-output

all: build

build: setup-env build-ui build-server

setup-env:
	@if [ ! -f .env ]; then cp .env.example .env; echo "Created .env from .env.example"; fi

build-ui:
	npm ci --prefix ui
	npm run build --prefix ui -- --outDir ../$(OUTPUT_DIR)/ui --emptyOutDir

build-server: setup-env
	cargo build --release
	mkdir -p $(OUTPUT_DIR)
	cp target/release/$(BINARY) $(OUTPUT_DIR)/$(BINARY)
	cp .env $(OUTPUT_DIR)/.env

run: build
	@set -a && . ./.env && set +a && ./$(OUTPUT_DIR)/$(BINARY)

clean:
	cargo clean
	rm -rf $(OUTPUT_DIR) ui/dist

clean-output:
	rm -rf $(OUTPUT_DIR)
