# spoons as a container: the MCP server over HTTP, for running behind mcp-gateway, which authenticates callers and
# injects each user's receipt address as the X-Spoons-Email header.
#
# No build stage: CI builds the static musl binaries and this image only copies them in. The build context is
# dist/, holding <arch>/spoons for amd64 and arm64 (the release tarballs, unpacked) and an empty state/, so
# building by hand needs `docker build -f Dockerfile dist` with those in place.

FROM scratch

ARG TARGETARCH

# rustls-platform-verifier reads the system trust store on Linux and has no fallback, so without a CA bundle on
# disk every HTTPS request fails. Copied from distroless/static, which needs neither a package manager nor a build
# stage.
COPY --from=gcr.io/distroless/static:latest \
     /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

COPY ${TARGETARCH}/spoons /spoons

# Quotes and payments. Lost with the container, which is safe: a quote that no longer exists can't be paid, and a
# payment nobody pays lapses by itself.
COPY --chown=10001:10001 state /state

EXPOSE 8080

# scratch has no /etc/passwd, so a numeric uid.
USER 10001:10001

LABEL org.opencontainers.image.title="spoons-mcp"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.source="https://github.com/radiosilence/spoons-mcp"

ENTRYPOINT ["/spoons"]
CMD ["mcp", "--http", "0.0.0.0:8080", "--state", "/state"]
