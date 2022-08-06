FROM rust:1.60

WORKDIR /checker

COPY rust-toolchain.toml ./rust-toolchain.toml

RUN rustup show

COPY Cargo.toml ./Cargo.toml

RUN cargo fetch

COPY src ./src

RUN cargo build


