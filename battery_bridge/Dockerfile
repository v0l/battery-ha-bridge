# Build context is the REPO ROOT (see .github/workflows/builder.yml).
# HA pulls the prebuilt image from GHCR (config.yaml `image:`), so the
# Supervisor never builds locally -- CI is the only builder and it has the
# whole checkout, so we COPY sources instead of cloning.
FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev dbus-dev pkgconf

# musl targets default to static linking, but libdbus is only available as a
# shared library on Alpine; link dynamically (runtime image ships dbus-libs).
ENV RUSTFLAGS="-C target-feature=-crt-static"

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# `can` enables the Linux-only Pylontech SocketCAN transport.
RUN cargo build --release --locked --features can \
    && strip target/release/battery-ha-bridge

FROM alpine:3.21
RUN apk add --no-cache curl jq libgcc dbus-libs
COPY --from=builder /build/target/release/battery-ha-bridge /usr/local/bin/
COPY battery_bridge/run.sh /
RUN chmod a+x /run.sh
CMD [ "/run.sh" ]
