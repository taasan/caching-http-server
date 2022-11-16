FROM rust:1-bullseye as builder

RUN export DEBIAN_FRONTEND=noninteractive \
    && apt-get update \
    && apt-get install --no-install-recommends -y emacs-nox \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/myapp

COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY . .
RUN make install

FROM debian:bullseye-slim
COPY --from=builder /usr/local/cargo/bin/caching-http-server /usr/local/bin/caching-http-server
ENTRYPOINT ["caching-http-server"]
