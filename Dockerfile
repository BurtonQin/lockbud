FROM rust as builder

WORKDIR /opt/lockbud

RUN env USER=root cargo init .

COPY Cargo.toml .
COPY Cargo.lock .

RUN mkdir .cargo
RUN cargo vendor > .cargo/config

COPY src /opt/lockbud/src

ARG RUST_VERSION=nightly-2023-04-11

RUN cd /opt/lockbud/ && \
    rustup default ${RUST_VERSION} && \
    rustup component add rust-src && \
    rustup component add rustc-dev && \
    rustup component add llvm-tools-preview && \
    cargo install --locked --path . && \
    rm -rf /opt/lockbud/ && \
    rm -rf /usr/local/cargo/registry/

FROM rust:slim

ARG RUST_VERSION=nightly-2023-04-11

RUN rustup default ${RUST_VERSION}

COPY --from=builder /usr/local/cargo/bin/cargo-lockbud /usr/local/cargo/bin/cargo-lockbud
COPY --from=builder /usr/local/cargo/bin/lockbud /usr/local/cargo/bin/lockbud

WORKDIR /volume

COPY entrypoint.sh /usr/local/bin/
ENTRYPOINT ["entrypoint.sh"]
