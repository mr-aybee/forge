# syntax=docker/dockerfile:1
# forge.orbyntech.com: the website at / and the documentation at /docs/.
# Build from the repository root:
#   docker build -f website/Dockerfile -t forge-website .

FROM node:24-alpine AS website
WORKDIR /src
COPY LICENSE LICENSE
COPY branding branding
WORKDIR /src/website
COPY website/package.json website/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY website/ ./
RUN npm run build

FROM node:24-alpine AS docs
WORKDIR /src
# The docs theme imports the website's brand tokens.
COPY website/src/styles/tokens.css website/src/styles/tokens.css
WORKDIR /src/docs
COPY docs/package.json docs/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY docs/ ./
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.29-alpine
COPY website/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=website /src/website/dist /usr/share/nginx/html
COPY --from=docs /src/docs/dist /usr/share/nginx/html/docs
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s CMD wget -qO- http://127.0.0.1:8080/healthz || exit 1
