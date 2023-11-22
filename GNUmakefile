.PHONY: build
build: tangle
	cargo build --release

.PHONY: install
install: tangle
	cargo install --locked --offline --frozen --path .

.PHONY: run
run: tangle
	cargo run

.PHONY: clippy
clippy: tangle
	cargo clippy -- -D warnings

.PHONY: lint
lint: clippy

.PHONY: tangle
tangle: README.org src
	emacs --batch --eval "(require 'org)" --eval '(org-babel-tangle-file "README.org")'

src:
	mkdir $@

caching-http-server.src.tar.gz: tangle
	tar cfz $@ Cargo.toml src $$(git ls-files)
