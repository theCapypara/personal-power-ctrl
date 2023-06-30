FROM rust:slim-buster as builder

RUN apt-get update && apt-get install -y \
  libssl-dev \
  pkg-config \
  libglib2.0-dev \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN USER=root cargo new --bin personal-power-ctrl
WORKDIR /src/personal-power-ctrl
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
RUN cargo build --release  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD . ./

RUN rm ./target/release/deps/personal_power_ctrl*

RUN cargo build --release
RUN strip /src/personal-power-ctrl/target/release/personal-power-ctrl


FROM rust:slim-buster as build

ARG APP=/app

EXPOSE 34434

ENV TZ=Etc/UTC \
    APP_USER=personal_power_ctrl \
    RUST_LOG="personal_power_ctrl=info"

RUN adduser --system --group $APP_USER

RUN apt-get update && apt-get install -y \
  ca-certificates \
  tzdata \
  libglib2.0 \
  && rm -rf /var/lib/apt/lists/*


COPY --from=builder /src/personal-power-ctrl/target/release/personal-power-ctrl ${APP}/personal-power-ctrl

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}
STOPSIGNAL INT

ENTRYPOINT ["./personal-power-ctrl"]
