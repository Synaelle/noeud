# syntax=docker/dockerfile:1
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The server image: one binary on distroless, no shell, not root, the
# database in the /data volume. Build from the repository root:
#   docker build -f deploy/Dockerfile -t noeud-server .

FROM rust:1.99.0-trixie@sha256:81015672d6939b0265f09adf96d5de125482bb1330106076956b0518e74edc90 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY server server
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p noeud-server \
 && install -D target/release/noeud-server /out/bin/noeud-server \
 && install -d -m 700 /out/root/data

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab
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
