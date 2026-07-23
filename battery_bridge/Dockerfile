FROM rust:alpine AS builder

RUN apk add --no-cache git musl-dev dbus-dev pkgconf

# musl targets default to static linking, but libdbus is only available as a
# shared library on Alpine; link dynamically (runtime image ships dbus-libs).
ENV RUSTFLAGS="-C target-feature=-crt-static"

# The add-on build context contains only this directory, so clone the crate;
# pin with BRIDGE_REF for reproducible builds. battery_control comes via a
# cargo git dependency pinned in Cargo.lock.
ARG BRIDGE_REF=main
RUN git clone --depth 1 --branch "$BRIDGE_REF" \
        https://github.com/v0l/battery-ha-bridge /build
WORKDIR /build
# `can` enables the Linux-only Pylontech SocketCAN transport.
RUN cargo build --release --locked --features can \
    && strip target/release/battery-ha-bridge

FROM alpine:3.21
RUN apk add --no-cache curl jq libgcc dbus-libs
COPY --from=builder /build/target/release/battery-ha-bridge /usr/local/bin/
COPY run.sh /
RUN chmod a+x /run.sh
CMD [ "/run.sh" ]
