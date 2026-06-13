# The exact shape that broke real-world Rust worker builds: a multi-stage
# Dockerfile where one ARG selects the binary and another optionally adds
# features via an if/else RUN, then the artifact is staged at a fixed path
# so the runtime stage's COPY doesn't need to re-expand the ARG.
ARG BASE_IMAGE=ghcr.io/blackleafdigital/zlayer-test-alpine:latest

FROM ${BASE_IMAGE} AS builder
ARG APP_NAME=zapp
ARG APP_FEATURES=""
WORKDIR /src
RUN if [ -n "${APP_FEATURES}" ]; then \
        printf 'built %s with features %s\n' "${APP_NAME}" "${APP_FEATURES}" > "/tmp/${APP_NAME}" ; \
    else \
        printf 'built %s default\n' "${APP_NAME}" > "/tmp/${APP_NAME}" ; \
    fi \
    && cp "/tmp/${APP_NAME}" /usr/local/bin/zapp-artifact

FROM ${BASE_IMAGE}
COPY --from=builder /usr/local/bin/zapp-artifact /usr/local/bin/zapp-artifact
CMD ["cat", "/usr/local/bin/zapp-artifact"]
