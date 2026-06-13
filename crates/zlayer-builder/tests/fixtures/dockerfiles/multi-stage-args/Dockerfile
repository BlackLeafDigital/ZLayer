# ARG scoping across stages: a global (pre-FROM) ARG is visible in FROM
# lines of every stage, but a stage-local ARG must be re-declared to be
# visible in that stage's RUN. COPY --from moves the artifact across.
ARG BASE_TAG=latest

FROM ghcr.io/blackleafdigital/zlayer-test-alpine:${BASE_TAG} AS producer
ARG MESSAGE=default-message
RUN printf '%s\n' "${MESSAGE}" > /artifact.txt

FROM ghcr.io/blackleafdigital/zlayer-test-alpine:${BASE_TAG}
# MESSAGE is deliberately NOT re-declared here: per Docker semantics it
# expands empty in this stage, which the build asserts.
RUN test -z "${MESSAGE}"
COPY --from=producer /artifact.txt /artifact.txt
RUN cp /artifact.txt /final.txt
CMD ["cat", "/final.txt"]
