# Metadata + filesystem instructions: ENV expansion inside RUN, WORKDIR
# creation, LABEL/EXPOSE/HEALTHCHECK config, ENTRYPOINT+CMD interplay.
FROM ghcr.io/blackleafdigital/zlayer-test-alpine:latest
ENV APP_ENV=production \
    APP_PORT=8080
WORKDIR /app/data
RUN pwd > /app/workdir.txt && printf '%s:%s\n' "$APP_ENV" "$APP_PORT" > /app/env.txt
LABEL com.zlayer.test="dockerfile-compat"
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s CMD ["true"]
ENTRYPOINT ["/bin/sh", "-c"]
CMD ["cat /app/env.txt"]
