# Multi-stage build: ~20 MB runtime image with the binary, configs and UI.
#
#   docker build -t tickrail .
#   docker run --rm -p 8080:8080 tickrail run --set http.listen=0.0.0.0:8080
#   docker run --rm -p 8080:8080 -v $PWD/data:/app/data tickrail run -c config/binance.toml --set http.listen=0.0.0.0:8080

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p tickrail-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/tickrail /usr/local/bin/tickrail
COPY config ./config
COPY ui ./ui
COPY examples ./examples
RUN useradd --system --home /app tickrail && mkdir -p /app/data && chown tickrail /app/data
USER tickrail
EXPOSE 8080
ENTRYPOINT ["tickrail"]
CMD ["run", "--set", "http.listen=0.0.0.0:8080"]
