FROM golang:1.20-alpine AS builder

RUN apk add git patch

RUN git clone https://chromium.googlesource.com/infra/goma/server

# This patch disables authentication
COPY remoteexec_patch_noauth.diff .
RUN cd server && patch -p1 < ../remoteexec_patch_noauth.diff

RUN cd server/cmd/remoteexec_proxy/ && go mod download && go build -o /remoteexec_proxy

FROM golang:1.20-alpine

COPY --from=builder /remoteexec_proxy /remoteexec_proxy

EXPOSE 5050
