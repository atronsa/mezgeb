# The ledger service, mezgeb-api, with the operator's command, mezgeb-cli.
#
#   docker build -t mezgeb-api .
#   docker run --rm mezgeb-api mezgeb-cli migrate   # as mezgeb_owner
#   docker run -p 8080:8080 mezgeb-api              # as mezgeb_app
#
# Configuration comes from the environment (crates/api/src/config.rs).
# Clients and credentials live in the database (`mezgeb-cli client`, as
# mezgeb_admin); no secret is baked in. compose.yaml runs it locally.

FROM rust:1.96-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY migrations migrations
RUN cargo build --release --locked -p mezgeb-api -p mezgeb-cli

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --no-create-home mezgeb
COPY --from=build /src/target/release/mezgeb-api /src/target/release/mezgeb-cli /usr/local/bin/
COPY charts /etc/mezgeb/charts
ENV MEZGEB_CHART=/etc/mezgeb/charts/wallet-provider.toml \
    MEZGEB_LISTEN=0.0.0.0:8080
USER mezgeb
EXPOSE 8080
CMD ["mezgeb-api"]
