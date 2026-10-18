# The DataSea ingestion service. Run it behind an HTTPS endpoint (any container host that terminates
# TLS) with CRANE_MONGODB_URI set as a secret; Crane installations upload to it with no setup.
# bookworm in both stages, so the binary links against the glibc it runs with
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release --features server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 crane
COPY --from=build /src/target/release/crane /usr/local/bin/crane
USER crane
ENV PORT=8080
EXPOSE 8080
CMD ["crane", "server"]
