FROM rust:1-bookworm AS tectonic-builder
ARG TECTONIC_VERSION=0.16.9
ARG TEX_FMT_VERSION=0.5.7

RUN apt-get update && apt-get install -y --no-install-recommends \
    libfontconfig1-dev \
    libgraphite2-dev \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install tectonic --version "${TECTONIC_VERSION}" --locked
RUN cargo install tex-fmt --version "${TEX_FMT_VERSION}" --locked

FROM texlive/texlive:latest
WORKDIR /app

# Install runtime tools from the distribution package manager. Avoid piping
# downloaded installers into a shell in the production image path.
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ca-certificates \
    chktex \
    ghostscript \
    && rm -rf /var/lib/apt/lists/*

COPY --from=tectonic-builder /usr/local/cargo/bin/tectonic /usr/local/bin/tectonic
COPY --from=tectonic-builder /usr/local/cargo/bin/tex-fmt /usr/local/bin/tex-fmt

# Copy easytex server binary (compiled locally on host)
COPY target/release/easytex /usr/local/bin/easytex
RUN useradd --create-home --shell /usr/sbin/nologin easytex \
    && mkdir -p /app/projects \
    && chown -R easytex:easytex /app

# Configure networking and default ports
EXPOSE 8081
ENV PORT=8081
ENV ROOT_DIR=/app/projects
ENV RUST_LOG=info

# Default command to serve projects
USER easytex
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://localhost:8081/ready || exit 1
ENTRYPOINT ["easytex"]
CMD ["serve", "/app/projects"]
