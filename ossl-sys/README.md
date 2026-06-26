# ossl-sys

A crate providing low-level Rust bindings to the OpenSSL library.

`ossl` was spun off from the `kryoptic` project to provide a focused,
standalone set of bindings for interacting with OpenSSL's C API.

This crate provides unsafe FFI bindings. It is intended as a foundational
building block for higher-level cryptographic libraries rather than for
direct application use.

