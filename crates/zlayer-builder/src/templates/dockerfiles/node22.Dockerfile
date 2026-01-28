# Multi-stage build for Node.js 22
# Optimized for production with minimal image size

ARG NODE_ENV=production

FROM node:22-alpine AS builder
WORKDIR /app

# Install dependencies first for better layer caching
COPY package*.json ./
RUN npm ci --only=production && npm cache clean --force

FROM node:22-alpine
WORKDIR /app

# Copy production dependencies
COPY --from=builder /app/node_modules ./node_modules

# Copy application source
COPY . .

# Set environment
ENV NODE_ENV=${NODE_ENV}

# Run as non-root user
USER node

# Default command - override with your entry point
CMD ["node", "index.js"]
