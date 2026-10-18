OUTPUT_DIR := Ouput

.PHONY: all build build-ui build-server run clean

all: build

build: build-ui build-server

build-ui:
	npm ci --prefix ui
	npm run build --prefix ui

build-server:
	CARGO_TARGET_DIR=$(OUTPUT_DIR) cargo build --release

run: build
	CARGO_TARGET_DIR=$(OUTPUT_DIR) cargo run --release

clean:
	rm -rf $(OUTPUT_DIR)