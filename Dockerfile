# Static musl build for `inspect`.
#
# Two-stage:
#   1. `build` — Alpine + musl + cargo, produces a fully-static binary.
#   2. `runtime` — distroless static; ships only the binary and an
#      ssh client (resolved at runtime by the user's host, not bundled).
#
# Note: `inspect` shells out to `ssh`, so the operator-facing runtime
# image installs OpenSSH client. Use it for `inspect` invocations only;
# do not exec into a long-running container.

ARG RUST_VERSION=1.82
ARG ALPINE_VERSION=3.20

FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS build
RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static ca-certificates git
WORKDIR /src
COPY . .
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --locked --target x86_64-unknown-linux-musl \
 && cp target/x86_64-unknown-linux-musl/release/inspect /usr/local/bin/inspect \
 && strip /usr/local/bin/inspect

FROM alpine:${ALPINE_VERSION} AS runtime
RUN apk add --no-cache openssh-client ca-certificates \
 && adduser -D -u 10001 inspect
COPY --from=build /usr/local/bin/inspect /usr/local/bin/inspect
USER inspect
WORKDIR /home/inspect
ENTRYPOINT ["/usr/local/bin/inspect"]
CMD ["--help"]
