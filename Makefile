WASM_TARGET = wasm32-unknown-unknown
CART_GAMES = hello snake

.PHONY: all carts carts-dir run run-desktop clean

all: carts

# === Build carts ===

carts: carts-dir $(patsubst %,carts/%.cart,$(CART_GAMES)) games/launcher/launcher.wasm

carts/%.cart: target/$(WASM_TARGET)/release/%.wasm | carts-dir
	cargo run -p cart-packer -- \
		--name "$(shell echo $* | sed 's/.*/\u&/')" \
		--wasm $< \
		-o $@

target/$(WASM_TARGET)/release/%.wasm: FORCE
	cargo build -p $* --target $(WASM_TARGET) --release

games/launcher/launcher.wasm: target/$(WASM_TARGET)/release/launcher.wasm
	wasm-opt -Oz --remove-unused-module-elements $< -o $@

carts-dir:
	@mkdir -p carts

# === Run on QEMU (default) ===

run: carts k23-embed
	cd deps/k23 && just run configurations/riscv64/qemu.toml --release

k23-embed: carts games/launcher/launcher.wasm
	cp games/launcher/launcher.wasm deps/k23/kernel/src/launcher.wasm
	cp carts/hello.cart deps/k23/kernel/src/hello.cart
	cp carts/snake.cart deps/k23/kernel/src/snake.cart

# === Run on desktop (simulator) ===

run-desktop: carts
	cargo run -p host-runner -- ./carts

# === Utilities ===

clean:
	cargo clean
	rm -rf carts games/launcher/launcher.wasm
	rm -f deps/k23/kernel/src/launcher.wasm deps/k23/kernel/src/*.cart

FORCE:
