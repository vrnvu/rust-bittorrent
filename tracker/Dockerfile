# Base image
FROM rust:latest as builder
WORKDIR /build
COPY ./tracker/Cargo.lock ./tracker/Cargo.toml ./

# Build dependencies (this will cache dependencies if unchanged)
RUN mkdir src \
    && echo "// dummy file" > src/lib.rs \
    && cargo build --release

COPY src src
RUN cargo build --locked --release
RUN cp ./target/release/$APP_NAME /bin/tracker

# Final stage: Use a minimal runtime image
FROM alpine:latest
COPY --from=builder /bin/tracker .

EXPOSE 3030

CMD ["./tracker"]
