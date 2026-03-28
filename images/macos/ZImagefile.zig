base: ghcr.io/blackleafdigital/zlayer/base:latest

steps:
  - run: apt-get install -y zig

labels:
  org.opencontainers.image.title: "ZLayer macOS Zig"
  org.opencontainers.image.vendor: "Black Leaf Digital"
