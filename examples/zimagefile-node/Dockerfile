# Dockerfile: Multi-stage Node.js application
#
# This is the traditional Dockerfile equivalent of the ZImagefile in this
# directory. Both produce the same image; compare them side-by-side to see
# how ZImagefile syntax maps to Dockerfile instructions.
#
# Build with:
#   docker build -t my-node-app:latest .
#   (or: zlayer build -t my-node-app:latest .)

# ---------- Stage 1: builder ----------
FROM node:22-alpine AS builder
WORKDIR /src

# Copy dependency manifests first for better layer caching
COPY package.json package-lock.json ./
RUN --mount=type=cache,target=/root/.npm,id=npm-cache,sharing=shared \
    npm ci

# Copy the rest of the source and build
COPY . .
RUN npm run build

# ---------- Stage 2: runtime ----------
FROM node:22-alpine AS runtime
WORKDIR /app

# Copy only production node_modules
COPY package.json package-lock.json ./
RUN --mount=type=cache,target=/root/.npm,id=npm-cache,sharing=shared \
    npm ci --omit=dev

# Copy compiled output from builder
COPY --from=builder /src/dist /app/dist

ENV NODE_ENV=production
EXPOSE 3000
USER node

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://localhost:3000/health || exit 1

CMD ["node", "dist/index.js"]
