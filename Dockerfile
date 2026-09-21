FROM node:20-alpine AS builder

WORKDIR /app

# Copy server package and install
COPY server/package*.json ./server/
RUN cd server && npm install

# Copy client package and install
COPY client/package*.json ./client/
RUN cd client && npm install

# Copy source codes
COPY server ./server
COPY client ./client

# Build client and server
RUN cd client && npm run build
RUN cd server && npm run build

FROM node:20-alpine AS runner

WORKDIR /app

COPY server/package*.json ./
RUN npm install --only=production

COPY --from=builder /app/server/dist ./dist
COPY --from=builder /app/client/dist /app/client/dist

EXPOSE 3001

ENV PORT=3001
ENV NODE_ENV=production

CMD ["node", "dist/index.js"]
