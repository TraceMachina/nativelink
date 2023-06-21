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

CMD ["/remoteexec_proxy", "--port", "5050", \
     "--remoteexec-addr", "nginx_internal:50051", \
     "--remote-instance-name", "default_instance", \
     "--insecure-remoteexec", \
     "--exec-missing-input-limit", "0", \
     "--exec-check-missing-timeout", "60s", \
     "--exec-check-cache-timeout", "10s", \
     "--exec-response-timeout", "30s", \
     "--exec-upload-blobs-timeout", "120s"]
