ARG BUILD_FROM
FROM $BUILD_FROM AS builder

# Build the bridge from source (the add-on build context contains only this
# directory, so we clone; pin with BRIDGE_REF for reproducible builds).
# battery-control is pulled by cargo as a git dependency.
ARG BRIDGE_REF=main
RUN apk add --no-cache rust cargo git musl-dev dbus-dev pkgconf

RUN git clone --depth 1 --branch "$BRIDGE_REF" \
        https://github.com/v0l/battery-ha-bridge /build/battery-ha-bridge
WORKDIR /build/battery-ha-bridge
# `can` enables the Linux-only Pylontech SocketCAN transport.
RUN cargo build --release --features can \
    && strip target/release/battery-ha-bridge

FROM $BUILD_FROM
RUN apk add --no-cache libgcc dbus-libs
COPY --from=builder /build/battery-ha-bridge/target/release/battery-ha-bridge /usr/local/bin/
COPY run.sh /
RUN chmod a+x /run.sh
CMD [ "/run.sh" ]
