# --- Stage 3: Runner Image ---
FROM texlive/texlive:latest
WORKDIR /app

# Install Tectonic (using drop-sh download script)
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ca-certificates \
    && curl --proto '=https' --tlsv1.2 -fsSL https://drop-sh.fullyjustified.net | sh \
    && mv tectonic /usr/local/bin/ \
    && curl -fsSL https://github.com/WGUNDERWOOD/tex-fmt/releases/latest/download/tex-fmt-x86_64-linux.tar.gz | tar -xz -C /usr/local/bin/ \
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
