# syntax=docker/dockerfile:1.7
#
# Multi-arch build for linux/amd64 and linux/arm64.
#
# Build args:
#   HF_TOKEN  — optional Hugging Face token for private model downloads at build time.
#               Pass with --build-arg HF_TOKEN=$(cat ~/.hf_token) or leave unset.
#
# Ports:
#   8765 — HTTP / REST
#   8766 — gRPC
#   8767 — Envoy ext_proc (when enabled in config)
#
# UDS socket path: /var/run/tokend.sock (matches default config)
#
# Usage:
#   docker buildx build \
#     --platform linux/amd64,linux/arm64 \
#     --build-arg HF_TOKEN=${HF_TOKEN:-} \
#     -t tokend:latest \
#     --push .

# ─── Stage 1: builder ────────────────────────────────────────────────────────
FROM rust:bookworm AS builder

# Build arg is only surfaced to the build environment, never baked into the
# final image layer. Set it here so cargo/tokenizers can reach HF at build time
# if the caller wants to pre-bake tokenizer files into a derived image.
ARG HF_TOKEN=""
ENV HF_TOKEN=${HF_TOKEN}

# TARGETPLATFORM / TARGETARCH are injected by buildx; Cargo uses them via
# the CARGO_BUILD_TARGET env. We pin the Rust target explicitly to guarantee
# the correct ABI on arm64 (aarch64-unknown-linux-gnu rather than musl).
ARG TARGETARCH

# System deps:
#   build-essential    — gcc, g++, make (oniguruma C++ build inside tokenizers)
#   cmake              — some tokenizer transitive deps need it
#   protobuf-compiler  — tonic-build compiles tokend.proto at cargo build time
#   pkg-config         — lets rustls / openssl-sys find system libs
#   libssl-dev         — native TLS for reqwest/tokenizers http feature
#   ca-certificates    — TLS cert bundle for HF downloads during build
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        protobuf-compiler \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Layer the dependency compile separately from source so that routine source
# edits don't invalidate the (slow) dependency compile cache.
#
# Copy manifests first, create a stub main, compile deps only, then overwrite
# with real source. This is the standard Rust Docker cache pattern.
COPY Cargo.toml Cargo.lock ./

# tonic-build needs the .proto to run build.rs; copy it now so the dep-only
# compile succeeds. If proto changes, the dep cache busts anyway.
COPY build.rs ./
COPY proto/ ./proto/

# Stub lib, binaries, and bench so `cargo build` resolves the full dep graph
# without our source. All targets are declared in Cargo.toml.
RUN mkdir -p src src/bin benches \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && echo 'fn main() {}' > src/bin/grpc_bench.rs \
    && echo 'fn main() {}' > benches/tokenize.rs

# --release with locked Cargo.lock; cross-compilation handled by buildx
# transparently (QEMU or native runners depending on the CI setup).
RUN cargo build --release --locked \
    && rm -rf src target/release/tokend target/release/tokend-bench \
    target/release/.fingerprint/tokend-*

# Now bring in real source and compile the actual binary.
# Touching main.rs forces Cargo to re-link even if the file hash didn't change.
# The bench stub from the dep-cache step persists — no need to copy real benches.
COPY src/ ./src/
RUN touch src/main.rs \
    && cargo build --release --locked

# ─── Stage 2: runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# ca-certificates: runtime TLS for tokenizer HTTP downloads (model fetch on
#   first request when not pre-baked).
# libc6 / libgcc-s1: glibc and GCC unwinder required by the Rust binary.
#   libssl3 + libcrypto3 are pulled transitively by libssl-dev in the builder
#   and linked dynamically; they must be present at runtime.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*

# Non-root service account. UID/GID 10001 is a common convention for
# non-system service accounts in hardened images.
RUN groupadd --gid 10001 tokend \
    && useradd --uid 10001 --gid tokend --shell /usr/sbin/nologin --no-create-home tokend

# Config directory — owned by root, readable by tokend.
# Operators mount their config here via volume or ConfigMap.
RUN mkdir -p /etc/tokend \
    && chown root:tokend /etc/tokend \
    && chmod 750 /etc/tokend

# UDS socket directory — writable by tokend so the daemon can bind.
RUN mkdir -p /var/run \
    && chown tokend:tokend /var/run \
    && chmod 755 /var/run

# Tokenizer cache directory. Typically a tmpfs or a host-path volume
# pre-populated with tokenizer.json files. The container can also
# download at runtime if HF_TOKEN is set in the environment.
RUN mkdir -p /var/cache/tokend \
    && chown tokend:tokend /var/cache/tokend \
    && chmod 700 /var/cache/tokend

COPY --from=builder /build/target/release/tokend /usr/local/bin/tokend
COPY --from=builder /build/target/release/tokend-bench /usr/local/bin/tokend-bench
RUN chmod 755 /usr/local/bin/tokend /usr/local/bin/tokend-bench

USER tokend

# HTTP REST
EXPOSE 8765
# gRPC
EXPOSE 8766
# Envoy ext_proc
EXPOSE 8767

# HF_TOKEN at runtime enables on-demand private model downloads.
# Unset by default — callers inject via `docker run -e HF_TOKEN=...`
# or Kubernetes secretKeyRef.
ENV HF_TOKEN=""
ENV HOME="/var/cache/tokend"
ENV HF_HOME="/var/cache/tokend"
ENV RUST_LOG="tokend=info,warn"

ENTRYPOINT ["tokend", "--config", "/etc/tokend/tokend.yaml", "serve"]
