.PHONY: build
build: tangle
	cargo build --release

.PHONY: install
install: tangle
	cargo install --path .

.PHONY: run
run: tangle
	cargo run

.PHONY: tangle
tangle: README.org src
	emacs --batch --eval "(require 'org)" --eval '(org-babel-tangle-file "README.org")'

src:
	mkdir $@

caching-http-server.src.tar.gz: tangle
	tar cfz $@ Cargo.toml src $$(git ls-files)
