FROM rustlang/rust:nightly as builder

WORKDIR /opt/lockbud

RUN env USER=root cargo init .

COPY Cargo.toml .
COPY Cargo.lock .

RUN mkdir .cargo
RUN cargo vendor > .cargo/config

COPY src /opt/lockbud/src

# FIXME: using rust-toolchain.toml, currently, we just pretent this ningtly env is work fine
RUN cd /opt/lockbud/ && \
    rustup component add rust-src && \
    rustup component add rustc-dev && \
    rustup component add llvm-tools-preview && \
    cargo install --locked --path . && \
    rm -rf /opt/lockbud/ && \
    rm -rf /usr/local/cargo/registry/

FROM rustlang/rust:nightly-slim

COPY --from=builder /usr/local/cargo/bin/cargo-lockbud /usr/local/cargo/bin/cargo-lockbud
COPY --from=builder /usr/local/cargo/bin/lockbud /usr/local/cargo/bin/lockbud

WORKDIR /volume

# TODO: using entrypoint, so that we can pass parameter
CMD cargo clean &&  cargo +nightly lockbud
