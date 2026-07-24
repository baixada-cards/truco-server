# syntax=docker/dockerfile:1.7

FROM rust:1.88.0-bookworm AS build

ARG SFW_VERSION=1.13.1
ARG SFW_SHA256=4dc46b626a7c5b81c0b54e1984ee53be5a628dbfb2f55ab14e9b04c8a134db6a
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl git \
    && rm -rf /var/lib/apt/lists/* \
    && curl --fail --location --proto '=https' --tlsv1.2 \
      "https://github.com/SocketDev/sfw-free/releases/download/v${SFW_VERSION}/sfw-free-linux-x86_64" \
      --output /usr/local/bin/sfw \
    && echo "${SFW_SHA256}  /usr/local/bin/sfw" | sha256sum --check - \
    && chmod 0755 /usr/local/bin/sfw

WORKDIR /src
COPY . .
RUN sfw cargo fetch --locked \
    && cargo build --locked --offline --release -p truco-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 65532 baixada \
    && useradd --system --uid 65532 --gid baixada --no-create-home baixada

ENV NODE_ENV=production
ENV PORT=8080

COPY --from=build /src/target/release/truco-server /usr/local/bin/truco-server

USER baixada
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/truco-server"]
