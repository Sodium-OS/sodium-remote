// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! UniFFI's binding generator, pinned to this crate's UniFFI version.
//! `remote/core/build-swift.sh` runs it.
fn main() {
    uniffi::uniffi_bindgen_main()
}
