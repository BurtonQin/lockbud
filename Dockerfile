FROM rustlang/rust:nightly as builder

WORKDIR /opt/lockbud

RUN env USER=root cargo init .

COPY . .

RUN cd /opt/lockbud/ && \
    cargo install --locked --path . && \
    rm -rf /usr/local/cargo/registry/

FROM rustlang/rust:nightly-slim

RUN apt-get update && \
    apt-get install -y curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/cargo-lockbud /usr/local/cargo/bin/cargo-lockbud
COPY --from=builder /usr/local/cargo/bin/lockbud /usr/local/cargo/bin/lockbud

CMD ["./detect.sh toys/inter"]