FROM golang:1.20-alpine

RUN apk add git patch

RUN git clone https://chromium.googlesource.com/infra/goma/server

# This patch removes the platform container from the request
COPY remoteexec_patch.diff .
RUN cd server && patch -p1 < ../remoteexec_patch.diff

# This patch disables authentication
COPY remoteexec_patch_noauth.diff .
RUN cd server && patch -p1 < ../remoteexec_patch_noauth.diff

RUN cd server/cmd/remoteexec_proxy/ && go mod download && go build -o /remoteexec_proxy

EXPOSE 5050
