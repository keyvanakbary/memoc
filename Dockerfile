FROM rust:1-alpine AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN cargo test --locked && cargo build --locked --release

FROM scratch AS runtime
ENV MEMOC_BIND=0.0.0.0:4000
USER 1000:1000
WORKDIR /docs
EXPOSE 4000
ENTRYPOINT ["/memoc"]

# CI supplies the native binary in an isolated build context.
FROM runtime AS prebuilt
COPY memoc /memoc

FROM runtime
COPY --from=build /build/target/release/memoc /memoc
