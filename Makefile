.PHONY: install types frontend front-build build dev-binary dev make-dev check test test-e2e test-granular doc doc-open docs clean

CARGO ?= $(HOME)/.cargo/bin/cargo
BUN ?= bun
BUNX ?= bunx

install:
	$(BUN) install
	cd frontend && $(BUN) install
	$(BUNX) playwright install chromium

types:
	$(CARGO) run -p xtask -- generate-types

frontend:
	cd frontend && $(BUN) run build

front-build: frontend

build: frontend
	$(CARGO) build --release

dev-binary: frontend
	$(CARGO) run -- serve example

dev:
	$(BUNX) concurrently -k -p "[{name}]" -n "Rust,Types,Vite" -c "cyan,magenta,yellow" \
		"$(CARGO) watch -w src -x 'run -- serve example'" \
		"$(CARGO) watch -w src -x 'run -p xtask -- generate-types'" \
		"cd frontend && $(BUN) run dev"

make-dev: dev

check: frontend
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

test:
	$(CARGO) test

test-e2e:
	$(BUN) test

test-granular:
	$(BUNX) playwright test tests/granular.spec.ts

doc:
	$(CARGO) doc --no-deps --workspace --document-private-items

doc-open:
	$(CARGO) doc --no-deps --workspace --document-private-items --open

docs: doc-open

clean:
	$(CARGO) clean
	rm -rf frontend/dist
	rm -rf target/
