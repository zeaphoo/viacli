# viacli 多架构编译 Dockerfile
# 支持: linux/amd64, linux/arm64

ARG REGISTRY=docker.m.daocloud.io/

FROM ${REGISTRY}rust:1.85-alpine3.21 AS build

ARG TARGETARCH

# 安装编译依赖
RUN apk add --no-cache \
    build-base \
    openssl-dev openssl-libs-static \
    ca-certificates

# 安装 musl 目标
RUN rustup target add x86_64-unknown-linux-musl \
    && rustup target add aarch64-unknown-linux-musl

WORKDIR /build

# 复制项目文件
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY .cargo ./.cargo

# 静态链接配置
ENV OPENSSL_DIR=/usr \
    OPENSSL_NO_VENDOR=1 \
    PKG_CONFIG_PATH=/usr/lib/pkgconfig \
    OPENSSL_STATIC=1 \
    PKG_CONFIG_ALL_STATIC=1 \
    RUSTFLAGS="-C target-feature=+crt-static"

# 根据目标架构编译
RUN case "${TARGETARCH}" in \
        arm64) \
            echo "Building for arm64..." && \
            cargo build --release --target aarch64-unknown-linux-musl && \
            mkdir -p /out && \
            cp /build/target/aarch64-unknown-linux-musl/release/viacli /out/viacli \
            ;; \
        amd64) \
            echo "Building for amd64..." && \
            cargo build --release --target x86_64-unknown-linux-musl && \
            mkdir -p /out && \
            cp /build/target/x86_64-unknown-linux-musl/release/viacli /out/viacli \
            ;; \
        *) \
            echo "unsupported TARGETARCH=${TARGETARCH}" >&2; \
            exit 1 \
            ;; \
    esac

RUN file /out/viacli && ls -lh /out/viacli

FROM scratch AS export

COPY --from=build /out/viacli /viacli
