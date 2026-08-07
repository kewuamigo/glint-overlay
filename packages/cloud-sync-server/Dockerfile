# Build: docker compose -f packages/cloud-sync-server/docker-compose.yml up --build
# Context is packages/ (sibling cloud-sync + this server).

FROM node:22-alpine AS build
WORKDIR /src

COPY cloud-sync/package.json cloud-sync/tsconfig.json ./cloud-sync/
COPY cloud-sync/src ./cloud-sync/src

COPY cloud-sync-server/package.json cloud-sync-server/tsconfig.json ./cloud-sync-server/
COPY cloud-sync-server/src ./cloud-sync-server/src

WORKDIR /src/cloud-sync
RUN npm install && npx tsc

WORKDIR /src/cloud-sync-server
RUN npm pkg set dependencies.@glint/cloud-sync="file:../cloud-sync" \
  && npm install \
  && npx tsc \
  && npm prune --omit=dev

FROM node:22-alpine
WORKDIR /app

COPY --from=build /src/cloud-sync /app/cloud-sync
COPY --from=build /src/cloud-sync-server /app/cloud-sync-server

ENV CLOUD_SYNC_HOST=0.0.0.0
ENV CLOUD_SYNC_PORT=8787
ENV CLOUD_SYNC_ROOT=/data

WORKDIR /app/cloud-sync-server
EXPOSE 8787
VOLUME ["/data"]

CMD ["node", "dist/cli.js"]
