#!/usr/bin/env sh

if [ "${NO_RUN}" != "1" ] && [ "${NO_RUN}" != "true" ]; then

    set -ex

    CARGO=cargo
    if [ "${CROSS}" = "1" ]; then
        export CARGO_NET_RETRY=5
        export CARGO_NET_TIMEOUT=10

        cargo install cross --git "https://github.com/cross-rs/cross" --rev "c7dee4d008475ce1c140773cbcd6078f4b86c2aa"
        CARGO=cross

        cargo clean
    fi

    # If a test crashes, we want to know which one it was.
    export RUST_TEST_THREADS=1
    export RUST_BACKTRACE=1

    # test monoio mod
    cd "${PROJECT_DIR}"/monoio

    # only enable legacy driver
    "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,legacy,macros,utils"
    "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,legacy,macros,utils" --release

    # enable legacy and sync
    "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,legacy,macros,utils,sync"

    # enable legacy and sync
    # TODO: fix linker error on loongarch64
    if [ "${TARGET}" != "loongarch64-unknown-linux-gnu" ]; then
        "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,legacy,macros,utils,sync" --release
    fi

    if [ "${TARGET}" = "x86_64-unknown-linux-gnu" ] || [ "${TARGET}" = "i686-unknown-linux-gnu" ]; then
        # only enabled uring driver
        "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,iouring,macros,utils"
        "${CARGO}" test --target "${TARGET}" --no-default-features --features "async-cancel,bytes,iouring,macros,utils" --release
    fi

    if [ "${OS}" = "windows-latest" ]; then
        # only enabled iocp driver
        # todo test it
        "${CARGO}" build --target "${TARGET}" --no-default-features --features "async-cancel,bytes,iocp,macros,utils"
        "${CARGO}" build --target "${TARGET}" --no-default-features --features "async-cancel,bytes,iocp,macros,utils" --release
    fi

    if [ "${TARGET}" != "aarch64-unknown-linux-gnu" ] && [ "${TARGET}" != "armv7-unknown-linux-gnueabihf" ] &&
        [ "${TARGET}" != "riscv64gc-unknown-linux-gnu" ] && [ "${TARGET}" != "s390x-unknown-linux-gnu" ] &&
        [ "${TARGET}" != "loongarch64-unknown-linux-gnu" ]; then
        # enable uring+legacy driver
        "${CARGO}" test --target "${TARGET}"
        "${CARGO}" test --target "${TARGET}" --release
    fi

    if [ "${CHANNEL}" = "nightly" ] && ([ "${TARGET}" = "x86_64-unknown-linux-gnu" ] || [ "${TARGET}" = "i686-unknown-linux-gnu" ]); then
        "${CARGO}" test --target "${TARGET}" --all-features
        "${CARGO}" test --target "${TARGET}" --all-features --release
    fi

    # test monoio-compat mod
    cd "${PROJECT_DIR}"/monoio-compat

    "${CARGO}" test --target "${TARGET}"
    "${CARGO}" test --target "${TARGET}" --release

    "${CARGO}" test --target "${TARGET}" --no-default-features --features hyper
    "${CARGO}" test --target "${TARGET}" --no-default-features --features hyper --release

    if [ "${CHANNEL}" = "nightly" ]; then
        "${CARGO}" test --target "${TARGET}" --all-features
        "${CARGO}" test --target "${TARGET}" --all-features --release
    fi

    # todo maybe we should test examples here ?
fi
