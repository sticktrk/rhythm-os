# https://developers.home-assistant.io/docs/add-ons/configuration#add-on-dockerfile
#
# When built by HA builder: BUILD_FROM is set from build.yaml
# When built via docker buildx: falls back to Alpine base image
ARG BUILD_FROM=alpine:3.21
FROM ${BUILD_FROM}

# Install runtime dependencies
RUN apk add --no-cache ca-certificates tzdata curl

# Map Docker platform arch (arm64, arm/v7) to our bin directory names (aarch64, armv7)
ARG TARGETARCH
ARG TARGETVARIANT
COPY bin/ /tmp/bin/
RUN set -e; \
    case "${TARGETARCH}${TARGETVARIANT:+/$TARGETVARIANT}" in \
        amd64)    ARCH=amd64 ;; \
        arm64)    ARCH=aarch64 ;; \
        arm/v7)   ARCH=armv7 ;; \
        *)        ARCH=${TARGETARCH} ;; \
    esac; \
    cp "/tmp/bin/${ARCH}/rhythm-addon" /usr/local/bin/rhythm-addon; \
    chmod +x /usr/local/bin/rhythm-addon; \
    rm -rf /tmp/bin

# Entrypoint: sets up HA env vars and execs the binary
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

CMD ["/entrypoint.sh"]
