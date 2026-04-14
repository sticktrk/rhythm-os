FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    bc \
    bison \
    build-essential \
    bzip2 \
    ca-certificates \
    cpio \
    diffutils \
    file \
    findutils \
    gawk \
    git \
    libncurses-dev \
    make \
    patch \
    perl \
    pkg-config \
    rsync \
    sed \
    tar \
    unzip \
    wget \
    which \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
