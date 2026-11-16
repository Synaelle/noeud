# syntax=docker/dockerfile:1
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The server image: one binary on distroless, no shell, not root, the
# database in the /data volume. Build from the repository root:
#   docker build -f deploy/Dockerfile -t noeud-server .

# The build runs on the machine's own platform and cross-compiles for the
# image's: much faster than emulating an ARM machine for the whole build.
FROM --platform=$BUILDPLATFORM rust:1.100.0-trixie@sha256:e19840879045c951c905d0eaa9887f3439c62000e429963371b376bac8d38c31 AS build
ARG TARGETARCH
ARG TARGETVARIANT
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY server server
# the Rust target, and the C compiler for SQLite and the linker for it
RUN case "$TARGETARCH$TARGETVARIANT" in \
      amd64) echo x86_64-unknown-linux-gnu x86_64-linux-gnu ;; \
      arm64) echo aarch64-unknown-linux-gnu aarch64-linux-gnu ;; \
      armv7) echo armv7-unknown-linux-gnueabihf arm-linux-gnueabihf ;; \
      *) echo "no build for $TARGETARCH$TARGETVARIANT" >&2; exit 1 ;; \
    esac > /target \
 && read -r target gnu < /target \
 && if [ "$(gcc -dumpmachine)" != "$gnu" ]; then \
      apt-get update && apt-get install -y --no-install-recommends "gcc-$(echo "$gnu" | tr _ -)" "libc6-dev-${TARGETARCH}${TARGETVARIANT:+hf}-cross" \
      && rm -rf /var/lib/apt/lists/*; \
    fi \
 && rustup target add "$target"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-$TARGETARCH$TARGETVARIANT,target=/src/target \
    read -r target gnu < /target \
 && triple=$(echo "$target" | tr a-z- A-Z_) \
 && export "CARGO_TARGET_${triple}_LINKER=$gnu-gcc" "CC_$(echo "$target" | tr - _)=$gnu-gcc" \
 && cargo build --release --locked -p noeud-server --target "$target" \
 && install -D "target/$target/release/noeud-server" /out/bin/noeud-server \
 && install -d -m 700 /out/root/data

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2
COPY --from=build /out/bin/ /usr/local/bin/
# the volume takes this folder's owner and mode when it is first made
COPY --from=build --chown=65532:65532 /out/root/ /
ENV NOEUD_DB=/data/noeud.db \
    NOEUD_LISTEN=0.0.0.0:8470
WORKDIR /data
VOLUME /data
EXPOSE 8470
USER 65532:65532
# no shell here: the server asks its own /v1/health
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/noeud-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/noeud-server"]
CMD ["serve"]
