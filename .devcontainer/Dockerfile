# Use Ubuntu as base since we're using features for Rust/Git/Nix
FROM mcr.microsoft.com/devcontainers/base:ubuntu

# Install git build dependencies and development tools that aren't available as features
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y \
        # Git build dependencies for testing different Git versions
        dh-autoreconf \
        libcurl4-gnutls-dev \
        libexpat1-dev \
        gettext \
        libz-dev \
        libssl-dev \
        # Additional development tools
        build-essential \
        pkg-config \
        # Tools for testing different Git versions
        autoconf \
        make \
        libpcre2-dev
