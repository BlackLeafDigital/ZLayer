base: ghcr.io/blackleafdigital/zlayer/base:latest

steps:
  - run: apt-get install -y swift

labels:
  org.opencontainers.image.title: "ZLayer macOS Swift"
  org.opencontainers.image.vendor: "Black Leaf Digital"
