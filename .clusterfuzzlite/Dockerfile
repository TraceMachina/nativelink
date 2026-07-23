FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:a96c8dfb5e0b4c418a3a63c6b1acd04a13d70335fbcaa370d4e1840780a58ce6

COPY . $SRC/nativelink
WORKDIR $SRC/nativelink
COPY .clusterfuzzlite/build.sh $SRC/
