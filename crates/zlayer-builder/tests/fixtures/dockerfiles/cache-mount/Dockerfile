# BuildKit-style RUN cache mount. The cache dir must exist and be
# writable during the RUN, and must NOT be part of the committed layer.
FROM ghcr.io/blackleafdigital/zlayer-test-alpine:latest
RUN --mount=type=cache,target=/var/cache/compat-e2e,id=dockerfile-compat-e2e,sharing=locked \
    echo stamp >> /var/cache/compat-e2e/stamp \
    && wc -l < /var/cache/compat-e2e/stamp | tr -d ' ' > /cache-runs.txt \
    && test -s /cache-runs.txt
CMD ["cat", "/cache-runs.txt"]
