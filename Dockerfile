FROM texlive/texlive:latest
WORKDIR /app

# Install runtime tools from the distribution package manager. Avoid piping
# downloaded installers into a shell in the production image path.
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ca-certificates \
    chktex \
    ghostscript \
    tectonic \
    && rm -rf /var/lib/apt/lists/*

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
ENTRYPOINT ["easytex", "serve", "/app/projects"]
