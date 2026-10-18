OUTPUT_DIR := Output
BINARY := Formicary

.PHONY: all build build-ui build-server run clean

all: build

build: build-ui build-server

build-ui:
	npm ci --prefix ui
	npm run build --prefix ui

build-server:
	cargo build --release
	mkdir -p $(OUTPUT_DIR)
	cp target/release/$(BINARY) $(OUTPUT_DIR)/$(BINARY)

run: build
	./$(OUTPUT_DIR)/$(BINARY)

clean:
	cargo clean
	rm -rf $(OUTPUT_DIR) ui/dist
